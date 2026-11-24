//! Text extractors for binary / markup documents: PDF (per page), DOCX (structure-aware
//! markdown) and HTML. The CPU-bound parsing runs on the blocking pool, guarded by size / page
//! limits, a timeout and `catch_unwind` (the PDF library panics on some malformed files).

use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use tokio::sync::Semaphore;

use crate::application::extract::{
    DOCX_MIME, ExtractError, ExtractInput, ExtractedText, ExtractorChain, MarkdownExtractor,
    NoopOcr, OcrEngine, OcrTextExtractor, PDF_MIME, PlainTextExtractor, TextExtractor,
};
use crate::domain::PageText;

/// Resource limits for extraction.
#[derive(Debug, Clone)]
pub struct ExtractLimits {
    /// Largest accepted file.
    pub max_bytes: usize,
    /// Most PDF pages accepted.
    pub max_pdf_pages: usize,
    /// Zip-bomb guard: largest uncompressed `word/document.xml`.
    pub max_docx_xml_bytes: u64,
    /// Wall-clock budget of one PDF / DOCX extraction.
    pub timeout: Duration,
    /// Extractions running at once (each pins a blocking thread, also after a timeout).
    pub max_concurrent: usize,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_bytes: 50 * 1024 * 1024,
            max_pdf_pages: 500,
            max_docx_xml_bytes: 64 * 1024 * 1024,
            timeout: Duration::from_secs(120),
            max_concurrent: 2,
        }
    }
}

/// Plain text, markdown, HTML, PDF, DOCX and images (OCR). `ocr` also handles scanned PDFs.
pub fn full_chain(limits: ExtractLimits, ocr: Arc<dyn OcrEngine>) -> ExtractorChain {
    let permits = Arc::new(Semaphore::new(limits.max_concurrent.max(1)));
    ExtractorChain::new(vec![
        Arc::new(MarkdownExtractor),
        Arc::new(PlainTextExtractor),
        Arc::new(HtmlExtractor),
        Arc::new(PdfExtractor::with_permits(
            limits.clone(),
            ocr.clone(),
            permits.clone(),
        )),
        Arc::new(DocxExtractor::with_permits(limits, permits)),
        Arc::new(OcrTextExtractor::new(ocr)),
    ])
}

/// [`full_chain`] without an OCR engine (scanned PDFs / images report `OcrUnavailable`).
pub fn default_chain(limits: ExtractLimits) -> ExtractorChain {
    full_chain(limits, Arc::new(NoopOcr))
}

fn has_ext(name: Option<&str>, ext: &str) -> bool {
    name.is_some_and(|n| n.to_ascii_lowercase().ends_with(ext))
}

/// Run `f` on the blocking pool under the limits' timeout, turning panics into errors.
async fn run_blocking<T, F>(
    permits: &Arc<Semaphore>,
    timeout: Duration,
    what: &'static str,
    f: F,
) -> Result<T, ExtractError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ExtractError> + Send + 'static,
{
    let permit = permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ExtractError::Failed("extractor shut down".into()))?;
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        catch_unwind(AssertUnwindSafe(f))
    });
    match tokio::time::timeout(timeout, handle).await {
        Err(_) => Err(ExtractError::Failed(format!(
            "{what} extraction timed out after {}s",
            timeout.as_secs()
        ))),
        Ok(Err(e)) => Err(ExtractError::Failed(format!(
            "{what} extraction aborted: {e}"
        ))),
        Ok(Ok(Err(_panic))) => Err(ExtractError::Failed(format!(
            "{what} parser crashed on this file"
        ))),
        Ok(Ok(Ok(r))) => r,
    }
}

// ---- PDF ---------------------------------------------------------------------------------

pub struct PdfExtractor {
    limits: ExtractLimits,
    ocr: Arc<dyn OcrEngine>,
    permits: Arc<Semaphore>,
}

impl PdfExtractor {
    pub fn new(limits: ExtractLimits, ocr: Arc<dyn OcrEngine>) -> Self {
        let permits = Arc::new(Semaphore::new(limits.max_concurrent.max(1)));
        Self::with_permits(limits, ocr, permits)
    }

    fn with_permits(
        limits: ExtractLimits,
        ocr: Arc<dyn OcrEngine>,
        permits: Arc<Semaphore>,
    ) -> Self {
        Self {
            limits,
            ocr,
            permits,
        }
    }
}

#[async_trait]
impl TextExtractor for PdfExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        mime == PDF_MIME || mime == "application/x-pdf" || has_ext(file_name, ".pdf")
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        if input.bytes.len() > self.limits.max_bytes {
            return Err(ExtractError::Failed(format!(
                "PDF is {} bytes, over the {} byte limit",
                input.bytes.len(),
                self.limits.max_bytes
            )));
        }
        let head = &input.bytes[..input.bytes.len().min(1024)];
        if !head.windows(5).any(|w| w == b"%PDF-") {
            return Err(ExtractError::Failed(
                "not a valid PDF (no %PDF- header)".into(),
            ));
        }
        let bytes = input.bytes.to_vec();
        let max_pages = self.limits.max_pdf_pages;
        let raw = run_blocking(&self.permits, self.limits.timeout, "PDF", move || {
            let pages = pdf_extract::extract_text_from_mem_by_pages(&bytes)
                .map_err(|e| ExtractError::Failed(format!("PDF could not be read: {e}")))?;
            if pages.len() > max_pages {
                return Err(ExtractError::Failed(format!(
                    "PDF has {} pages, over the {max_pages} page limit",
                    pages.len()
                )));
            }
            Ok(pages)
        })
        .await?;
        match pages_from_pdf(&raw) {
            Some(pages) => Ok(ExtractedText { pages }),
            // No text layer: scanned document, hand over to OCR.
            None => Ok(ExtractedText {
                pages: self.ocr.recognize(input.bytes, PDF_MIME).await?,
            }),
        }
    }
}

/// Normalised pages (1-based numbers kept even when a page is empty), or `None` when the
/// document has no text layer (fewer than ~10 letters / digits per page on average).
pub fn pages_from_pdf(raw: &[String]) -> Option<Vec<PageText>> {
    let pages: Vec<PageText> = raw
        .iter()
        .enumerate()
        .map(|(i, t)| PageText {
            page: Some(i as u32 + 1),
            text: normalize_pdf_text(t),
        })
        .collect();
    let alnum: usize = pages
        .iter()
        .map(|p| p.text.chars().filter(|c| c.is_alphanumeric()).count())
        .sum();
    if pages.is_empty() || alnum < 10 * pages.len() {
        return None;
    }
    Some(pages.into_iter().filter(|p| !p.text.is_empty()).collect())
}

fn expand_ligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        _ => return None,
    })
}

/// Collapse whitespace, expand ligatures, re-join words hyphenated across a line break
/// (`inter-\nnational` -> `international`, only when the next line starts lower-case) and
/// limit blank lines to one.
pub fn normalize_pdf_text(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.replace("\r\n", "\n").chars() {
        match c {
            '\0' => {}
            '\u{00A0}' | '\u{200B}' | '\t' => cleaned.push(' '),
            '\r' | '\u{000C}' => cleaned.push('\n'),
            c => match expand_ligature(c) {
                Some(s) => cleaned.push_str(s),
                None => cleaned.push(c),
            },
        }
    }
    let lines: Vec<String> = cleaned
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let mut out = String::with_capacity(cleaned.len());
    let mut blank = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        i += 1;
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        let mut cur = line.clone();
        // Merge hyphenated line breaks (possibly several in a row).
        while let (true, Some(next)) = (
            ends_with_soft_hyphen(&cur),
            lines.get(i).filter(|n| starts_lowercase(n)),
        ) {
            cur.pop();
            cur.push_str(next);
            i += 1;
        }
        out.push_str(&cur);
    }
    out
}

fn ends_with_soft_hyphen(s: &str) -> bool {
    let mut it = s.chars().rev();
    it.next() == Some('-') && it.next().is_some_and(char::is_alphabetic)
}

fn starts_lowercase(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_lowercase)
}

// ---- DOCX --------------------------------------------------------------------------------

pub struct DocxExtractor {
    limits: ExtractLimits,
    permits: Arc<Semaphore>,
}

impl DocxExtractor {
    pub fn new(limits: ExtractLimits) -> Self {
        let permits = Arc::new(Semaphore::new(limits.max_concurrent.max(1)));
        Self::with_permits(limits, permits)
    }

    fn with_permits(limits: ExtractLimits, permits: Arc<Semaphore>) -> Self {
        Self { limits, permits }
    }
}

#[async_trait]
impl TextExtractor for DocxExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        mime == DOCX_MIME || has_ext(file_name, ".docx")
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        if input.bytes.len() > self.limits.max_bytes {
            return Err(ExtractError::Failed(format!(
                "DOCX is {} bytes, over the {} byte limit",
                input.bytes.len(),
                self.limits.max_bytes
            )));
        }
        let bytes = input.bytes.to_vec();
        let max_xml = self.limits.max_docx_xml_bytes;
        let text = run_blocking(&self.permits, self.limits.timeout, "DOCX", move || {
            docx_to_markdown(&bytes, max_xml)
        })
        .await?;
        let pages = if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![PageText { page: None, text }]
        };
        Ok(ExtractedText { pages })
    }
}

/// Markdown-ish text of a `.docx`: headings as `#`, list items as `- `, tables as lines of
/// ` | `-joined cells. Drawings and text boxes are ignored.
pub fn docx_to_markdown(bytes: &[u8], max_xml_bytes: u64) -> Result<String, ExtractError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ExtractError::Failed(format!("not a valid DOCX (zip): {e}")))?;
    if zip.len() > 20_000 {
        return Err(ExtractError::Failed(
            "DOCX archive has too many entries".into(),
        ));
    }
    let entry = zip
        .by_name("word/document.xml")
        .map_err(|_| ExtractError::Failed("DOCX has no word/document.xml".into()))?;
    if entry.size() > max_xml_bytes {
        return Err(ExtractError::Failed(format!(
            "DOCX document.xml expands to {} bytes, over the {max_xml_bytes} byte limit",
            entry.size()
        )));
    }
    // The declared size can lie: cap what is actually inflated.
    let mut xml = Vec::new();
    entry
        .take(max_xml_bytes + 1)
        .read_to_end(&mut xml)
        .map_err(|e| ExtractError::Failed(format!("DOCX could not be inflated: {e}")))?;
    if xml.len() as u64 > max_xml_bytes {
        return Err(ExtractError::Failed(format!(
            "DOCX document.xml expands past the {max_xml_bytes} byte limit"
        )));
    }
    parse_document_xml(&xml)
}

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn attr_val(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| local(a.key.as_ref()) == b"val")
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// `Heading1`..`Heading9` (any case, optional space), `Title` -> 1; also `heading 2`.
fn heading_level(style: &str) -> Option<usize> {
    let s: String = style
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if s == "title" {
        return Some(1);
    }
    let n = s.strip_prefix("heading")?;
    n.parse::<usize>().ok().map(|n| n.clamp(1, 6))
}

#[derive(Default)]
struct DocxState {
    out: String,
    para: String,
    style_level: Option<usize>,
    list_item: bool,
    in_text: bool,
    /// Depth inside `w:drawing` / `w:pict` (ignored content).
    skip: usize,
    table_depth: usize,
    rows: Vec<Vec<String>>,
    cell: Option<String>,
}

impl DocxState {
    fn finish_paragraph(&mut self) {
        let text = self.para.split_whitespace().collect::<Vec<_>>().join(" ");
        let level = self.style_level.take();
        let list = std::mem::take(&mut self.list_item);
        self.para.clear();
        if text.is_empty() {
            return;
        }
        if self.table_depth > 0 {
            if let Some(cell) = self.cell.as_mut() {
                if !cell.is_empty() {
                    cell.push(' ');
                }
                cell.push_str(&text);
            }
            return;
        }
        self.block_break();
        match (level, list) {
            (Some(l), _) => {
                self.out.push_str(&"#".repeat(l));
                self.out.push(' ');
            }
            (None, true) => self.out.push_str("- "),
            _ => {}
        }
        self.out.push_str(&text);
        self.out.push('\n');
    }

    fn block_break(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            // Consecutive list items stay tight.
            if !(self.list_item_run()) {
                self.out.push('\n');
            }
        }
    }

    fn list_item_run(&self) -> bool {
        self.list_item
            && self
                .out
                .rsplit('\n')
                .nth(1)
                .is_some_and(|l| l.starts_with("- "))
    }

    fn flush_table(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        let lines: Vec<String> = rows
            .iter()
            .map(|r| r.join(" | "))
            .filter(|l| !l.replace('|', "").trim().is_empty())
            .collect();
        if lines.is_empty() {
            return;
        }
        self.block_break();
        self.out.push_str(&lines.join("\n"));
        self.out.push('\n');
    }
}

fn parse_document_xml(xml: &[u8]) -> Result<String, ExtractError> {
    let mut reader = Reader::from_reader(xml);
    let mut st = DocxState::default();
    let mut buf = Vec::new();
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| ExtractError::Failed(format!("DOCX XML is malformed: {e}")))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => {
                let name = e.name();
                match local(name.as_ref()) {
                    b"drawing" | b"pict" | b"object" => st.skip += 1,
                    _ if st.skip > 0 => {}
                    b"p" => {
                        st.para.clear();
                        st.style_level = None;
                        st.list_item = false;
                    }
                    b"pStyle" => st.style_level = attr_val(&e).as_deref().and_then(heading_level),
                    b"numPr" => st.list_item = true,
                    b"t" => st.in_text = true,
                    b"tbl" => {
                        st.table_depth += 1;
                        if st.table_depth == 1 {
                            st.rows.clear();
                        }
                    }
                    b"tr" if st.table_depth == 1 => st.rows.push(Vec::new()),
                    b"tc" if st.table_depth == 1 => st.cell = Some(String::new()),
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let name = e.name();
                if st.skip == 0 {
                    match local(name.as_ref()) {
                        b"pStyle" => {
                            st.style_level = attr_val(&e).as_deref().and_then(heading_level);
                        }
                        b"numPr" => st.list_item = true,
                        b"tab" | b"ptab" => st.para.push(' '),
                        b"br" | b"cr" => st.para.push('\n'),
                        b"noBreakHyphen" => st.para.push('-'),
                        _ => {}
                    }
                }
            }
            Event::Text(t) if st.in_text && st.skip == 0 => {
                let s = t
                    .unescape()
                    .map_err(|e| ExtractError::Failed(format!("DOCX XML is malformed: {e}")))?;
                st.para.push_str(&s);
            }
            Event::End(e) => {
                let name = e.name();
                match local(name.as_ref()) {
                    b"drawing" | b"pict" | b"object" => st.skip = st.skip.saturating_sub(1),
                    _ if st.skip > 0 => {}
                    b"t" => st.in_text = false,
                    b"p" => st.finish_paragraph(),
                    b"tc" if st.table_depth == 1 => {
                        if let (Some(cell), Some(row)) = (st.cell.take(), st.rows.last_mut()) {
                            row.push(cell);
                        }
                    }
                    b"tbl" => {
                        st.table_depth = st.table_depth.saturating_sub(1);
                        if st.table_depth == 0 {
                            st.flush_table();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(st.out.trim().to_owned())
}

// ---- HTML --------------------------------------------------------------------------------

/// `text/html`: tags dropped, `<h1>`..`<h6>` become markdown headings, block elements become
/// paragraphs, `<script>` / `<style>` / comments are removed, entities decoded.
#[derive(Debug, Clone, Copy, Default)]
pub struct HtmlExtractor;

#[async_trait]
impl TextExtractor for HtmlExtractor {
    fn supports(&self, mime: &str, file_name: Option<&str>) -> bool {
        mime == "text/html"
            || mime == "application/xhtml+xml"
            || has_ext(file_name, ".html")
            || has_ext(file_name, ".htm")
    }

    async fn extract(&self, input: &ExtractInput<'_>) -> Result<ExtractedText, ExtractError> {
        let s = std::str::from_utf8(input.bytes).map_err(|_| ExtractError::InvalidEncoding)?;
        let text = html_to_text(s.trim_start_matches('\u{feff}'));
        let pages = if text.is_empty() {
            Vec::new()
        } else {
            vec![PageText { page: None, text }]
        };
        Ok(ExtractedText { pages })
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent
                    .strip_prefix('#')
                    .and_then(|n| match n.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok(),
                        None => n.parse().ok(),
                    })
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    let mut skip_until: Option<&'static str> = None;
    while let Some(lt) = rest.find('<') {
        if skip_until.is_none() {
            out.push_str(&decode_entities(&rest[..lt]));
        }
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if let Some(until) = skip_until {
            if closing && name == until {
                skip_until = None;
            }
            continue;
        }
        match (name.as_str(), closing) {
            ("script", false) => skip_until = Some("script"),
            ("style", false) => skip_until = Some("style"),
            ("head", false) => skip_until = Some("head"),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                let level = usize::from(name.as_bytes()[1] - b'0');
                out.push_str("\n\n");
                out.push_str(&"#".repeat(level));
                out.push(' ');
            }
            ("li", false) => out.push_str("\n- "),
            ("td" | "th", true) => out.push_str(" | "),
            (
                "p" | "div" | "br" | "tr" | "table" | "ul" | "ol" | "section" | "article" | "h1"
                | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "blockquote",
                _,
            ) => out.push('\n'),
            _ => {}
        }
    }
    if skip_until.is_none() {
        out.push_str(&decode_entities(rest));
    }
    tidy_lines(&out)
}

/// Trim lines, drop ` | ` row tails, cap blank lines at one.
fn tidy_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0usize;
    for line in s.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = line.trim_end_matches(" |").trim_end_matches('|').trim_end();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::domain::{ChunkingParams, chunk_document};

    const TWO_PAGES: &[u8] = include_bytes!("../../tests/fixtures/two_pages.pdf");
    const SCANNED: &[u8] = include_bytes!("../../tests/fixtures/scanned.pdf");

    fn chain() -> ExtractorChain {
        default_chain(ExtractLimits::default())
    }

    fn inp<'a>(bytes: &'a [u8], mime: &'a str, name: Option<&'a str>) -> ExtractInput<'a> {
        ExtractInput {
            bytes,
            mime,
            file_name: name,
        }
    }

    fn docx(document_xml: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            z.start_file("[Content_Types].xml", o).unwrap();
            z.write_all(b"<Types/>").unwrap();
            z.start_file("word/document.xml", o).unwrap();
            z.write_all(document_xml.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn sample_docx() -> Vec<u8> {
        docx(&format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W}><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Chính sách hoàn tiền</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Khách hàng được hoàn tiền trong </w:t></w:r><w:r><w:t>30 ngày &amp; không cần lý do.</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Điều kiện</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Còn hóa đơn</w:t></w:r></w:p>
<w:p><w:r><w:t>Dòng một</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>Dòng hai</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>sau tab</w:t></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline xmlns:wp="x"><w:p><w:r><w:t>HIDDEN</w:t></w:r></w:p></wp:inline></w:drawing></w:r></w:p>
<w:tbl>
<w:tr><w:tc><w:p><w:r><w:t>Gói</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Giá</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Cơ bản</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>100.000đ</w:t></w:r></w:p></w:tc></w:tr>
</w:tbl>
<w:p><w:r><w:t>Hết.</w:t></w:r></w:p>
</w:body></w:document>"#
        ))
    }

    #[tokio::test]
    async fn docx_structure_vietnamese_tables_and_drawings() {
        let bytes = sample_docx();
        // Routed by magic bytes even though the declared type is wrong.
        let r = chain()
            .extract(&inp(&bytes, "application/octet-stream", Some("x.bin")))
            .await
            .unwrap();
        assert_eq!(r.pages.len(), 1);
        let t = &r.pages[0].text;
        assert!(t.starts_with("# Chính sách hoàn tiền\n\n"), "{t}");
        assert!(t.contains("30 ngày & không cần lý do."), "{t}");
        assert!(t.contains("## Điều kiện\n\n- Còn hóa đơn"), "{t}");
        assert!(t.contains("Dòng một Dòng hai sau tab"), "{t}");
        assert!(t.contains("Gói | Giá\nCơ bản | 100.000đ"), "{t}");
        assert!(!t.contains("HIDDEN"), "{t}");
        assert!(t.ends_with("Hết."), "{t}");
    }

    #[tokio::test]
    async fn docx_headings_feed_the_chunker() {
        let bytes = sample_docx();
        let r = chain()
            .extract(&inp(&bytes, super::DOCX_MIME, None))
            .await
            .unwrap();
        let chunks = chunk_document(&r.pages, &ChunkingParams::default());
        assert!(
            chunks
                .iter()
                .any(|c| c.heading_path == vec!["Chính sách hoàn tiền", "Điều kiện"]),
            "{chunks:?}"
        );
    }

    #[tokio::test]
    async fn docx_zip_bomb_and_garbage_are_errors() {
        let big = format!(
            "<w:document {W}><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>",
            "a".repeat(200_000)
        );
        let bytes = docx(&big);
        assert!(bytes.len() < 5_000, "should compress hard: {}", bytes.len());
        let small = DocxExtractor::new(ExtractLimits {
            max_docx_xml_bytes: 10_000,
            ..ExtractLimits::default()
        });
        let e = small
            .extract(&inp(&bytes, DOCX_MIME, None))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, ExtractError::Failed(m) if m.contains("limit")),
            "{e}"
        );
        // Not a zip at all.
        let e = small
            .extract(&inp(b"definitely not a zip", DOCX_MIME, None))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Failed(_)));
    }

    #[tokio::test]
    async fn pptx_and_xlsx_are_unsupported() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("ppt/presentation.xml", o).unwrap();
            z.write_all(b"<p/>").unwrap();
            z.finish().unwrap();
        }
        let e = chain()
            .extract(&inp(
                buf.get_ref(),
                "application/octet-stream",
                Some("a.pptx"),
            ))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Unsupported(_)), "{e}");
    }

    #[tokio::test]
    async fn pdf_pages_are_numbered_and_dehyphenated() {
        // Wrong file name / MIME: the %PDF- magic decides.
        let r = chain()
            .extract(&inp(TWO_PAGES, "text/plain", Some("notes.txt")))
            .await
            .unwrap();
        let pages: Vec<(Option<u32>, &str)> =
            r.pages.iter().map(|p| (p.page, p.text.as_str())).collect();
        assert_eq!(pages.len(), 2, "{pages:?}");
        assert_eq!(pages[0].0, Some(1));
        assert!(pages[0].1.contains("refund within 30 days"), "{pages:?}");
        assert_eq!(pages[1].0, Some(2));
        assert!(pages[1].1.contains("International orders"), "{pages:?}");
        // Page numbers reach the chunks (citations).
        let chunks = chunk_document(&r.pages, &ChunkingParams::default());
        assert!(
            chunks
                .iter()
                .any(|c| c.page == Some(1) && c.text.contains("warranty"))
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.page == Some(2) && c.text.contains("Orders ship"))
        );
    }

    #[tokio::test]
    async fn scanned_pdf_needs_ocr_and_uses_the_engine_when_present() {
        let e = chain()
            .extract(&inp(SCANNED, PDF_MIME, None))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::OcrUnavailable), "{e}");

        struct Fake;
        #[async_trait]
        impl OcrEngine for Fake {
            async fn recognize(&self, _: &[u8], _: &str) -> Result<Vec<PageText>, ExtractError> {
                Ok(vec![PageText {
                    page: Some(1),
                    text: "ocr text".into(),
                }])
            }
        }
        let c = full_chain(ExtractLimits::default(), Arc::new(Fake));
        let r = c.extract(&inp(SCANNED, PDF_MIME, None)).await.unwrap();
        assert_eq!(r.pages[0].text, "ocr text");
    }

    #[tokio::test]
    async fn corrupt_pdf_is_an_error_not_a_panic() {
        let mut bad = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 9 0 R >>\nendobj\n".to_vec();
        bad.extend_from_slice(&[0xff; 300]);
        for bytes in [
            bad.as_slice(),
            b"%PDF-1.7 garbage".as_slice(),
            &TWO_PAGES[..TWO_PAGES.len() / 2],
        ] {
            let e = chain()
                .extract(&inp(bytes, PDF_MIME, None))
                .await
                .unwrap_err();
            assert!(matches!(e, ExtractError::Failed(_)), "{e}");
        }
        // Declared PDF without the header.
        let e = chain()
            .extract(&inp(b"hello", PDF_MIME, Some("a.pdf")))
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Failed(_)));
    }

    #[tokio::test]
    async fn pdf_size_and_page_limits() {
        let tiny = PdfExtractor::new(
            ExtractLimits {
                max_bytes: 100,
                ..ExtractLimits::default()
            },
            Arc::new(NoopOcr),
        );
        let e = tiny
            .extract(&inp(TWO_PAGES, PDF_MIME, None))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, ExtractError::Failed(m) if m.contains("byte limit")),
            "{e}"
        );
        let one_page = PdfExtractor::new(
            ExtractLimits {
                max_pdf_pages: 1,
                ..ExtractLimits::default()
            },
            Arc::new(NoopOcr),
        );
        let e = one_page
            .extract(&inp(TWO_PAGES, PDF_MIME, None))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, ExtractError::Failed(m) if m.contains("page limit")),
            "{e}"
        );
    }

    #[tokio::test]
    async fn panics_and_timeouts_become_errors() {
        let permits = Arc::new(Semaphore::new(1));
        let r: Result<(), _> = run_blocking(&permits, Duration::from_secs(5), "T", || {
            panic!("boom");
        })
        .await;
        assert!(matches!(&r, Err(ExtractError::Failed(m)) if m.contains("crashed")));
        let r: Result<(), _> = run_blocking(&permits, Duration::from_millis(50), "T", || {
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        })
        .await;
        assert!(matches!(&r, Err(ExtractError::Failed(m)) if m.contains("timed out")));
    }

    #[test]
    fn pdf_text_normalisation() {
        assert_eq!(
            normalize_pdf_text("inter-\nnational   trade\u{FB01}\n\n\n\nNext-\nCase"),
            "international tradefi\n\nNext-\nCase"
        );
        assert_eq!(normalize_pdf_text("a\t b\r\nc"), "a b\nc");
        assert!(pages_from_pdf(&["".into(), " ".into()]).is_none());
    }

    #[tokio::test]
    async fn html_is_converted() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><h1>Tiêu đề</h1>\
                    <p>Một &amp; hai&nbsp;ba</p><script>var a=1;</script><ul><li>x</li><li>y</li></ul>\
                    <table><tr><td>a</td><td>b</td></tr></table><!-- c --></body></html>";
        let r = chain()
            .extract(&inp(html.as_bytes(), "text/html", None))
            .await
            .unwrap();
        assert_eq!(
            r.pages[0].text,
            "# Tiêu đề\n\nMột & hai ba\n\n- x\n- y\n\na | b"
        );
    }
}
