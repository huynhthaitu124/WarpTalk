//! Glossary entities, validation rules and CSV handling (spec 12 section 5.7, BR-57, BR-98). Pure.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, WorkspaceId};

/// Terms and translations (spec 12 section 5.7).
pub const MAX_TERM_CHARS: usize = 200;
pub const MAX_NAME_CHARS: usize = 150;
pub const MAX_DOMAIN_CHARS: usize = 100;
pub const MAX_NOTES_CHARS: usize = 1_000;
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_LANGUAGE_CHARS: usize = 15;
/// Rows accepted by one bulk import request.
pub const MAX_IMPORT_ROWS: usize = 2_000;
/// Published global terms (prompt-injection bound, spec 12 section 5.7 code constant).
pub const MAX_PUBLISHED_GLOBAL_TERMS: i64 = 200;
pub const DEFAULT_PRIORITY: i32 = 5;

#[derive(Debug, Clone)]
pub struct Glossary {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub term_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct GlossaryTerm {
    pub id: Uuid,
    pub glossary_id: Uuid,
    pub source_term: String,
    pub target_term: String,
    pub case_sensitive: bool,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalStatus {
    Draft,
    Published,
    Archived,
}

impl GlobalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Self::Draft),
            "published" => Some(Self::Published),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlobalTerm {
    pub id: Uuid,
    pub term: String,
    pub preferred_translation: String,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
    pub business_domain: Option<String>,
    pub definition: Option<String>,
    pub usage_note: Option<String>,
    pub priority: i32,
    pub status: GlobalStatus,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl GlobalTerm {
    /// Content stored in audit rows (`before_json` / `after_json`).
    pub fn audit_json(&self) -> Value {
        serde_json::json!({
            "term": self.term,
            "preferredTranslation": self.preferred_translation,
            "sourceLanguage": self.source_language,
            "targetLanguage": self.target_language,
            "businessDomain": self.business_domain,
            "definition": self.definition,
            "usageNote": self.usage_note,
            "priority": self.priority,
            "status": self.status.as_str(),
            "version": self.version,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GlobalAudit {
    pub id: Uuid,
    pub term_id: Uuid,
    pub action: String,
    pub before_json: Option<Value>,
    pub after_json: Option<Value>,
    pub actor_user_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// One term of a Redis snapshot read by the pipeline (`glossary:workspace:{id}`, `glossary:global`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTerm {
    pub source: String,
    pub target: String,
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub case_sensitive: bool,
}

fn bad(code: &str, msg: impl Into<String>) -> AppError {
    AppError::validation(code, msg)
}

/// Language codes such as `en`, `vi`, `zh-CN`: 2-3 letters then optional alphanumeric subtags.
pub fn validate_language(raw: &str) -> AppResult<String> {
    let s = raw.trim();
    let mut parts = s.split('-');
    let base = parts.next().unwrap_or_default();
    let ok = (2..=3).contains(&base.len())
        && base.chars().all(|c| c.is_ascii_alphabetic())
        && parts
            .all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
        && s.len() <= MAX_LANGUAGE_CHARS;
    if !ok {
        return Err(bad(
            "INVALID_LANGUAGE",
            format!("`{s}` is not a language code such as en, vi or ja"),
        ));
    }
    let base_lc = base.to_ascii_lowercase();
    Ok(format!("{base_lc}{}", &s[base.len()..]))
}

fn text_field(name: &str, raw: &str, max: usize) -> AppResult<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(bad("INVALID_VALUE", format!("{name} must not be empty")));
    }
    if s.chars().count() > max {
        return Err(bad(
            "VALUE_TOO_LONG",
            format!("{name} must be at most {max} characters"),
        ));
    }
    if s.chars().any(char::is_control) {
        return Err(bad(
            "INVALID_VALUE",
            format!("{name} must not contain control characters"),
        ));
    }
    Ok(s.to_owned())
}

fn optional_text(name: &str, raw: Option<&str>, max: usize) -> AppResult<Option<String>> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if s.chars().count() > max => Err(bad(
            "VALUE_TOO_LONG",
            format!("{name} must be at most {max} characters"),
        )),
        Some(s) => Ok(Some(s.to_owned())),
    }
}

pub fn validate_name(raw: &str) -> AppResult<String> {
    text_field("name", raw, MAX_NAME_CHARS)
}

pub fn validate_description(raw: Option<&str>) -> AppResult<Option<String>> {
    optional_text("description", raw, MAX_DESCRIPTION_CHARS)
}

pub fn validate_term_pair(source: &str, target: &str) -> AppResult<(String, String)> {
    Ok((
        text_field("sourceTerm", source, MAX_TERM_CHARS)?,
        text_field("targetTerm", target, MAX_TERM_CHARS)?,
    ))
}

pub fn validate_notes(raw: Option<&str>) -> AppResult<Option<String>> {
    optional_text("notes", raw, MAX_NOTES_CHARS)
}

/// Validated global term fields.
#[derive(Debug, Clone)]
pub struct GlobalTermFields {
    pub term: String,
    pub preferred_translation: String,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
    pub business_domain: Option<String>,
    pub definition: Option<String>,
    pub usage_note: Option<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, Default)]
pub struct GlobalTermInput<'a> {
    pub term: &'a str,
    pub preferred_translation: &'a str,
    pub source_language: Option<&'a str>,
    pub target_language: Option<&'a str>,
    pub business_domain: Option<&'a str>,
    pub definition: Option<&'a str>,
    pub usage_note: Option<&'a str>,
    pub priority: Option<i32>,
}

pub fn validate_global_term(i: &GlobalTermInput<'_>) -> AppResult<GlobalTermFields> {
    let lang = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(validate_language)
            .transpose()
    };
    let priority = i.priority.unwrap_or(DEFAULT_PRIORITY);
    if !(1..=10).contains(&priority) {
        return Err(bad("INVALID_VALUE", "priority must be between 1 and 10"));
    }
    Ok(GlobalTermFields {
        term: text_field("term", i.term, MAX_TERM_CHARS)?,
        preferred_translation: text_field(
            "preferredTranslation",
            i.preferred_translation,
            MAX_TERM_CHARS,
        )?,
        source_language: lang(i.source_language)?,
        target_language: lang(i.target_language)?,
        business_domain: optional_text("businessDomain", i.business_domain, MAX_DOMAIN_CHARS)?,
        definition: optional_text("definition", i.definition, MAX_DESCRIPTION_CHARS)?,
        usage_note: optional_text("usageNote", i.usage_note, MAX_NOTES_CHARS)?,
        priority,
    })
}

// ---------------------------------------------------------------- CSV

/// Splits one CSV record; fields may be double-quoted with `""` as an escaped quote.
fn split_csv_line(line: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    let mut quoted_field = false;
    while let Some(c) = chars.next() {
        match (c, in_quotes) {
            ('"', true) => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            }
            ('"', false) if cur.is_empty() && !quoted_field => {
                in_quotes = true;
                quoted_field = true;
            }
            (',', false) => {
                out.push(std::mem::take(&mut cur));
                quoted_field = false;
            }
            (c, _) => cur.push(c),
        }
    }
    if in_quotes {
        return None;
    }
    out.push(cur);
    Some(out)
}

/// Result of parsing an import payload.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedImport {
    /// Unique (case-insensitive on source), validated pairs in input order.
    pub pairs: Vec<(String, String)>,
    /// Rows dropped as duplicates of an earlier row of the same payload.
    pub duplicates_in_payload: usize,
}

/// Parses `source,target` lines. A first line `source,target` is a header. Blank lines are
/// ignored; more than [`MAX_IMPORT_ROWS`] data rows is rejected; any invalid row rejects the
/// whole payload (all-or-nothing). Duplicates inside the payload keep the first row.
pub fn parse_import_csv(text: &str) -> AppResult<ParsedImport> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    if let Some((_, first)) = rows.first() {
        let norm: String = first
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '"')
            .collect::<String>()
            .to_ascii_lowercase();
        if norm == "source,target" {
            rows.remove(0);
        }
    }
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(bad(
            "IMPORT_TOO_LARGE",
            format!(
                "at most {MAX_IMPORT_ROWS} rows per import, got {}",
                rows.len()
            ),
        ));
    }
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(rows.len());
    let mut seen = std::collections::HashSet::new();
    let mut dup = 0;
    for (n, line) in rows {
        let fields = split_csv_line(line)
            .filter(|f| f.len() == 2)
            .ok_or_else(|| bad("INVALID_CSV", format!("line {n}: expected `source,target`")))?;
        let (s, t) = validate_term_pair(&fields[0], &fields[1])
            .map_err(|e| bad("INVALID_CSV", format!("line {n}: {e}")))?;
        if seen.insert(s.to_lowercase()) {
            pairs.push((s, t));
        } else {
            dup += 1;
        }
    }
    Ok(ParsedImport {
        pairs,
        duplicates_in_payload: dup,
    })
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// `source,target` CSV with a header row.
pub fn render_csv<'a>(terms: impl IntoIterator<Item = &'a GlossaryTerm>) -> String {
    let mut out = String::from("source,target\n");
    for t in terms {
        out.push_str(&csv_field(&t.source_term));
        out.push(',');
        out.push_str(&csv_field(&t.target_term));
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn language_codes() {
        assert_eq!(validate_language(" EN ").unwrap(), "en");
        assert_eq!(validate_language("zh-CN").unwrap(), "zh-CN");
        assert!(validate_language("english").is_err());
        assert!(validate_language("e").is_err());
        assert!(validate_language("en_US").is_err());
    }

    #[test]
    fn csv_parse_dedupe_and_header() {
        let p = parse_import_csv("source,target\nHello,Xin chao\nhello,Chao\n\"a,b\",\"c\"\"d\"\n")
            .unwrap();
        assert_eq!(p.duplicates_in_payload, 1);
        assert_eq!(
            p.pairs,
            vec![
                ("Hello".to_owned(), "Xin chao".to_owned()),
                ("a,b".to_owned(), "c\"d".to_owned())
            ]
        );
    }

    #[test]
    fn csv_rejects_bad_rows_and_limit() {
        assert!(parse_import_csv("only-one\n").is_err());
        assert!(parse_import_csv("a,\n").is_err());
        let big: String = (0..=MAX_IMPORT_ROWS).map(|i| format!("s{i},t\n")).collect();
        assert_eq!(
            parse_import_csv(&big).unwrap_err().code(),
            "IMPORT_TOO_LARGE"
        );
        let ok: String = (0..MAX_IMPORT_ROWS).map(|i| format!("s{i},t\n")).collect();
        assert_eq!(parse_import_csv(&ok).unwrap().pairs.len(), MAX_IMPORT_ROWS);
    }

    #[test]
    fn csv_render_round_trips() {
        let t = GlossaryTerm {
            id: Uuid::nil(),
            glossary_id: Uuid::nil(),
            source_term: "a,b".into(),
            target_term: "q\"x".into(),
            case_sensitive: false,
            notes: None,
            created_at: Utc::now(),
        };
        let csv = render_csv([&t]);
        let p = parse_import_csv(&csv).unwrap();
        assert_eq!(p.pairs, vec![("a,b".to_owned(), "q\"x".to_owned())]);
    }
}
