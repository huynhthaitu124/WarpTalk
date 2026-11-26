//! Text extraction port. Page structure is part of the contract: extractors return one
//! [`PageText`] per page (PDF page markers) so chunks can cite `page`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::{PageText, split_pages_by_markers};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ExtractError {
    #[error("unsupported content type `{0}`")]
    Unsupported(String),
    #[error("content is not valid UTF-8 text")]
    InvalidEncoding,
    /// The content needs OCR (image or scanned PDF) and no OCR engine is configured.
    #[error("OCR is not available")]
    OcrUnavailable,
    /// OCR would send the document to an external model but the workspace forbids it.
    #[error("OCR needs an external model but externalLlmAllowed is off")]
    ExternalLlmDisallowed,
    #[error("extraction failed: {0}")]
    Failed(String),
    /// A backend the extraction depends on is temporarily unavailable (retried by the indexer).
    #[error("extraction backend unavailable: {0}")]
    Unavailable(String),
}

pub struct ExtractInput<'a> {
    pub bytes: &'a [u8],
    pub mime: &'a str,
    pub file_name: Option<&'a str>,
    /// Workspace `externalLlmAllowed`: engines that send content to a third party must refuse
    /// when false.
    pub external_llm_allowed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedText {
    pub pages: Vec<PageText>,
}

#[async_trait]
pub trait TextExtractor: Send + Sync {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool;
    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError>;
}

/// OCR engine port (images, scanned PDFs).
///
/// Engines live in `infrastructure::ocr` (Tesseract CLI, OpenAI vision). [`NoopOcr`] makes image
/// / scanned documents fail with [`ExtractError::OcrUnavailable`] (spec 14 gap G6).
#[async_trait]
pub trait OcrEngine: Send + Sync {
    /// Recognise text, one entry per page / image.
    async fn recognize(&self, bytes: &[u8], mime: &str) -> Result<Vec<PageText>, ExtractError>;

    /// True when document content leaves the platform (hosted vision model). Such engines are
    /// skipped for workspaces with `externalLlmAllowed = false`.
    fn sends_content_externally(&self) -> bool {
        false
    }
}

/// Placeholder OCR engine: always reports that OCR is unavailable.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopOcr;

#[async_trait]
impl OcrEngine for NoopOcr {
    async fn recognize(&self, _bytes: &[u8], _mime: &str) -> Result<Vec<PageText>, ExtractError> {
        Err(ExtractError::OcrUnavailable)
    }
}

fn is_image(mime: &str, name: Option<&str>) -> bool {
    mime.starts_with("image/")
        || name.is_some_and(|n| {
            let n = n.to_ascii_lowercase();
            [
                ".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif", ".tif", ".tiff",
            ]
            .iter()
            .any(|e| n.ends_with(e))
        })
}

fn has_ext(name: Option<&str>, exts: &[&str]) -> bool {
    name.is_some_and(|n| {
        let n = n.to_ascii_lowercase();
        exts.iter().any(|e| n.ends_with(e))
    })
}

fn decode(bytes: &[u8]) -> Result<String, ExtractError> {
    let s = std::str::from_utf8(bytes).map_err(|_| ExtractError::InvalidEncoding)?;
    Ok(s.trim_start_matches('\u{feff}').replace("\r\n", "\n"))
}

/// Pages from text that may carry page markers (form feed, `<<page N>>`).
fn pages_of(text: String) -> Vec<PageText> {
    if text.contains('\u{000C}') || text.contains("<<page ") {
        split_pages_by_markers(&text)
    } else if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![PageText { page: None, text }]
    }
}

/// `text/plain` and `.txt`; also the hand-off format of external PDF/DOCX converters (pages
/// separated by form feed or `<<page N>>` lines).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainTextExtractor;

#[async_trait]
impl TextExtractor for PlainTextExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        mime == "text/plain" || has_ext(file_name, &[".txt", ".text", ".log"])
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        Ok(ExtractedText {
            pages: pages_of(decode(input.bytes)?),
        })
    }
}

/// `text/markdown` and `.md`: strips a leading YAML front matter block, keeps headings.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarkdownExtractor;

fn strip_front_matter(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("---\n") else {
        return s;
    };
    match rest.find("\n---\n") {
        Some(end) => &rest[end + 5..],
        None => s,
    }
}

#[async_trait]
impl TextExtractor for MarkdownExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        mime == "text/markdown"
            || mime == "text/x-markdown"
            || has_ext(file_name, &[".md", ".markdown"])
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        let text = decode(input.bytes)?;
        Ok(ExtractedText {
            pages: pages_of(strip_front_matter(&text).to_owned()),
        })
    }
}

/// Images (and anything else routed to OCR) through an [`OcrEngine`].
pub struct OcrTextExtractor {
    ocr: Arc<dyn OcrEngine>,
}

impl OcrTextExtractor {
    pub fn new(ocr: Arc<dyn OcrEngine>) -> Self {
        Self { ocr }
    }
}

#[async_trait]
impl TextExtractor for OcrTextExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        is_image(mime, file_name)
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        if self.ocr.sends_content_externally() && !input.external_llm_allowed {
            return Err(ExtractError::ExternalLlmDisallowed);
        }
        let pages = self.ocr.recognize(input.bytes, input.mime).await?;
        Ok(ExtractedText { pages })
    }
}

pub const PDF_MIME: &str = "application/pdf";
pub const DOCX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub const PPTX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";
pub const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Content type from magic bytes, when the bytes are conclusive: PNG / JPEG / WebP images (these
/// are routed to OCR whatever the declared type), `%PDF-` within the first KiB,
/// or a zip (`PK\x03\x04`) whose entry names (local headers at the start, central directory at
/// the end) identify OOXML word / presentation / spreadsheet packages.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        return Some(PDF_MIME);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if !bytes.starts_with(b"PK\x03\x04") {
        return None;
    }
    const WINDOW: usize = 1 << 20;
    let head = &bytes[..bytes.len().min(WINDOW)];
    let tail = &bytes[bytes.len().saturating_sub(WINDOW)..];
    let has = |n: &[u8]| contains(head, n) || contains(tail, n);
    if has(b"word/document.xml") {
        Some(DOCX_MIME)
    } else if has(b"ppt/presentation.xml") {
        Some(PPTX_MIME)
    } else if has(b"xl/workbook.xml") {
        Some(XLSX_MIME)
    } else {
        None
    }
}

/// First extractor that supports the input wins. The content type is decided by magic bytes
/// first (a `.txt` upload that is really a PDF is a PDF), then by MIME / file extension.
pub struct ExtractorChain {
    extractors: Vec<Arc<dyn TextExtractor>>,
}

impl ExtractorChain {
    pub fn new(extractors: Vec<Arc<dyn TextExtractor>>) -> Self {
        Self { extractors }
    }

    /// Plain text, markdown and (via [`NoopOcr`]) images that fail with `OcrUnavailable`.
    pub fn standard() -> Self {
        Self::new(vec![
            Arc::new(MarkdownExtractor),
            Arc::new(PlainTextExtractor),
            Arc::new(OcrTextExtractor::new(Arc::new(NoopOcr))),
        ])
    }

    pub async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        // A conclusive sniff overrides the declared type and the file name.
        let sniffed;
        let input = match sniff_mime(input.bytes) {
            Some(mime) => {
                sniffed = ExtractInput {
                    bytes: input.bytes,
                    mime,
                    file_name: None,
                    external_llm_allowed: input.external_llm_allowed,
                };
                &sniffed
            }
            None => input,
        };
        for e in &self.extractors {
            if e.supports(input.mime, input.file_name) {
                return e.extract(input).await;
            }
        }
        Err(ExtractError::Unsupported(input.mime.to_owned()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn input<'a>(bytes: &'a [u8], mime: &'a str, name: Option<&'a str>) -> ExtractInput<'a> {
        ExtractInput {
            bytes,
            mime,
            file_name: name,
            external_llm_allowed: true,
        }
    }

    #[tokio::test]
    async fn plain_text_pages_and_markdown_front_matter() {
        let chain = ExtractorChain::standard();
        let r = chain
            .extract(&input(b"one\r\n\x0ctwo", "text/plain", None))
            .await
            .unwrap();
        assert_eq!(r.pages.len(), 2);
        assert_eq!(r.pages[1].page, Some(2));
        let r = chain
            .extract(&input(b"just text", "", Some("a.txt")))
            .await
            .unwrap();
        assert_eq!(r.pages[0].page, None);
        let r = chain
            .extract(&input(
                b"---\ntitle: x\n---\n# Head\n\nBody.",
                "text/markdown",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.pages[0].text, "# Head\n\nBody.");
    }

    #[tokio::test]
    async fn images_report_ocr_unavailable_and_unknown_is_unsupported() {
        let chain = ExtractorChain::standard();
        let e = chain
            .extract(&input(&[0x89, 0x50], "image/png", None))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::OcrUnavailable));
        let e = chain
            .extract(&input(b"x", "application/zip", Some("a.zip")))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Unsupported(_)));
        let e = chain
            .extract(&input(&[0xff, 0xfe, 0xfd], "text/plain", None))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::InvalidEncoding));
    }
}
