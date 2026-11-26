//! Workspace document rules: file types, access levels, categories, naming. Pure Rust.

use serde::Serialize;
use warptalk_kernel::{AppError, AppResult, WorkspaceRole};

pub const MAX_TITLE_CHARS: usize = 255;
pub const MAX_FILE_NAME_CHARS: usize = 255;

/// Upload size limit unless configured otherwise (MB).
pub const DEFAULT_MAX_UPLOAD_MB: u64 = 20;

/// Who may see a document. Only active workspace members ever reach the check, so `Public` and
/// `MembersOnly` currently coincide (no guest access to documents); they stay distinct values
/// for the UI and a future guest/share-link feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AccessLevel {
    #[serde(rename = "PUBLIC")]
    Public,
    #[serde(rename = "MEMBERS_ONLY")]
    MembersOnly,
    #[serde(rename = "ADMIN_ONLY")]
    AdminOnly,
}

impl AccessLevel {
    pub const ALL: [AccessLevel; 3] = [Self::Public, Self::MembersOnly, Self::AdminOnly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "PUBLIC",
            Self::MembersOnly => "MEMBERS_ONLY",
            Self::AdminOnly => "ADMIN_ONLY",
        }
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PUBLIC" => Ok(Self::Public),
            "MEMBERS_ONLY" => Ok(Self::MembersOnly),
            "ADMIN_ONLY" => Ok(Self::AdminOnly),
            _ => Err(AppError::validation(
                "INVALID_ACCESS_LEVEL",
                "accessLevel must be PUBLIC, MEMBERS_ONLY or ADMIN_ONLY",
            )),
        }
    }

    pub fn visible_to(self, role: WorkspaceRole) -> bool {
        match self {
            Self::Public | Self::MembersOnly => true,
            Self::AdminOnly => matches!(role, WorkspaceRole::Owner | WorkspaceRole::Admin),
        }
    }
}

/// Access levels a role may see.
pub fn visible_levels(role: WorkspaceRole) -> Vec<AccessLevel> {
    AccessLevel::ALL
        .into_iter()
        .filter(|l| l.visible_to(role))
        .collect()
}

/// Document management (upload, patch, archive) is Owner/Admin only.
pub fn authorize_manage(role: WorkspaceRole) -> AppResult<()> {
    match role {
        WorkspaceRole::Owner | WorkspaceRole::Admin => Ok(()),
        WorkspaceRole::Member => Err(AppError::forbidden(
            "only owner or admin can manage documents",
        )),
    }
}

pub const CATEGORIES: [&str; 5] = [
    "GENERAL",
    "TECHNICAL",
    "FINANCIAL",
    "LEGAL",
    "MEETING_NOTES",
];

pub fn parse_category(s: &str) -> AppResult<&'static str> {
    let up = s.trim().to_ascii_uppercase();
    CATEGORIES.into_iter().find(|c| *c == up).ok_or_else(|| {
        AppError::validation(
            "INVALID_CATEGORY",
            "category must be GENERAL, TECHNICAL, FINANCIAL, LEGAL or MEETING_NOTES",
        )
    })
}

pub fn validate_title(title: &str) -> AppResult<String> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::validation(
            "INVALID_TITLE",
            format!("title must be 1-{MAX_TITLE_CHARS} characters"),
        ));
    }
    Ok(t.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocType {
    Pdf,
    Docx,
    Txt,
    Md,
    Csv,
    /// Images are OCR'd by the knowledge indexer (when an OCR engine is configured).
    Png,
    Jpg,
    Webp,
}

impl DocType {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            "txt" => Some(Self::Txt),
            "md" | "markdown" => Some(Self::Md),
            "csv" => Some(Self::Csv),
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub fn from_file_name(name: &str) -> Option<Self> {
        extension(name).and_then(Self::from_extension)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Txt => "txt",
            Self::Md => "md",
            Self::Csv => "csv",
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Webp => "webp",
        }
    }

    /// MIME type stored with the document (derived from the validated extension; the client's
    /// content type is not trusted).
    pub fn mime(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Txt => "text/plain",
            Self::Md => "text/markdown",
            Self::Csv => "text/csv",
            Self::Png => "image/png",
            Self::Jpg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    /// True when the index request can carry the text itself (no extractor needed).
    pub fn is_plain_text(self) -> bool {
        matches!(self, Self::Txt | Self::Md | Self::Csv)
    }

    /// Magic-byte / content sanity check.
    pub fn content_matches(self, bytes: &[u8]) -> bool {
        match self {
            // The PDF header may be preceded by a little junk (spec allows the first KiB).
            Self::Pdf => bytes[..bytes.len().min(1024)]
                .windows(5)
                .any(|w| w == b"%PDF-"),
            // OOXML is a zip archive holding `word/` parts.
            Self::Docx => {
                bytes.starts_with(b"PK\x03\x04") && bytes.windows(5).any(|w| w == b"word/")
            }
            Self::Txt | Self::Md | Self::Csv => decode_text(bytes).is_some(),
            Self::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpg => bytes.starts_with(b"\xFF\xD8\xFF"),
            Self::Webp => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        }
    }
}

/// UTF-8 text without NUL bytes (a leading BOM is dropped); `None` for binary content.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

/// Lower-cased extension of a file name, if it has a non-empty one.
pub fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then_some(ext)
}

/// Client-supplied file name reduced to a safe display name: directory parts and control
/// characters removed, at most 255 characters. Falls back to `document`.
pub fn sanitize_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?'))
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    let cleaned: String = cleaned.chars().take(MAX_FILE_NAME_CHARS).collect();
    if cleaned.is_empty() {
        "document".to_owned()
    } else {
        cleaned
    }
}

/// Default title: the file name without extension.
pub fn title_from_file_name(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    stem.chars().take(MAX_TITLE_CHARS).collect()
}

/// Ingestion states stored in `ingestion_status`.
pub mod ingestion {
    pub const PENDING: &str = "pending";
    pub const COMPLETED: &str = "completed";
    pub const SKIPPED: &str = "skipped";
    pub const FAILED: &str = "failed";
}

/// Which documents a bulk reindex picks up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReindexScope {
    Skipped,
    Failed,
    /// Skipped, failed and completed (a full refresh). Documents already `pending` are never
    /// touched.
    All,
}

impl ReindexScope {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            "all" => Ok(Self::All),
            _ => Err(AppError::validation(
                "INVALID_REINDEX_STATUS",
                "status must be skipped, failed or all",
            )),
        }
    }

    /// `ingestion_status` values the scope covers.
    pub fn statuses(self) -> &'static [&'static str] {
        match self {
            Self::Skipped => &[ingestion::SKIPPED],
            Self::Failed => &[ingestion::FAILED],
            Self::All => &[ingestion::SKIPPED, ingestion::FAILED, ingestion::COMPLETED],
        }
    }
}

/// Human readable size, e.g. `20 MB` (MSG47 text).
pub fn format_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB && bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn access_levels_by_role() {
        assert!(AccessLevel::AdminOnly.visible_to(WorkspaceRole::Admin));
        assert!(!AccessLevel::AdminOnly.visible_to(WorkspaceRole::Member));
        assert!(AccessLevel::MembersOnly.visible_to(WorkspaceRole::Member));
        assert_eq!(visible_levels(WorkspaceRole::Member).len(), 2);
        assert_eq!(visible_levels(WorkspaceRole::Owner).len(), 3);
        assert_eq!(
            AccessLevel::parse(" admin_only ").unwrap(),
            AccessLevel::AdminOnly
        );
        assert!(AccessLevel::parse("secret").is_err());
    }

    #[test]
    fn types_and_magic_bytes() {
        assert_eq!(DocType::from_file_name("a.PDF"), Some(DocType::Pdf));
        assert_eq!(DocType::from_file_name("a.exe"), None);
        assert_eq!(DocType::from_file_name("noext"), None);
        assert!(DocType::Pdf.content_matches(b"%PDF-1.7\n..."));
        assert!(!DocType::Pdf.content_matches(b"hello"));
        assert!(DocType::Docx.content_matches(b"PK\x03\x04....word/document.xml"));
        assert!(!DocType::Docx.content_matches(b"PK\x03\x04....xl/workbook.xml"));
        assert!(DocType::Txt.content_matches("héllo".as_bytes()));
        assert!(!DocType::Txt.content_matches(b"%PDF-\x00\x01"));
        assert!(!DocType::Csv.content_matches(&[0xff, 0xfe, 0x41]));
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi").unwrap(), "hi");
    }

    #[test]
    fn names() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("C:\\x\\report v1.pdf"), "report v1.pdf");
        assert_eq!(sanitize_file_name("  "), "document");
        assert_eq!(title_from_file_name("notes.v2.md"), "notes.v2");
        assert!(validate_title("   ").is_err());
        assert_eq!(parse_category("legal").unwrap(), "LEGAL");
        assert!(parse_category("x").is_err());
        assert_eq!(format_size(20 * 1024 * 1024), "20 MB");
    }
}
