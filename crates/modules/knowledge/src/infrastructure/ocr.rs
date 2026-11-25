//! OCR engines behind the [`OcrEngine`] port, selected with `KNOWLEDGE_OCR=none|tesseract|openai`.
//!
//! * `tesseract`: the `tesseract` CLI on images; scanned PDFs are rasterised page by page with
//!   the `pdftoppm` CLI (poppler). Processes are spawned with an argument vector (never through
//!   a shell), under a timeout, `kill_on_drop`, inside a private temp dir that is always removed.
//! * `openai`: a vision-capable chat model (`KNOWLEDGE_OCR_OPENAI_MODEL`, default `gpt-4o-mini`)
//!   reads each image / page. Document content leaves the platform, so the engine reports
//!   [`OcrEngine::sends_content_externally`] and is skipped for workspaces with
//!   `externalLlmAllowed = false`. `warptalk-llm` has no image input yet, hence the direct
//!   `reqwest` call here. UNVERIFIED against the live API: the request follows the documented
//!   chat-completions vision shape and is covered only by wiremock tests; the PDF-as-`file`
//!   fallback (used when `pdftoppm` is missing) is the least certain part.
//!
//! Missing binaries / keys are detected at startup ([`build_ocr`]): a WARN is logged and the
//! engine falls back to [`NoopOcr`] (documents end `skipped` with `ocr_unavailable`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::application::extract::{ExtractError, NoopOcr, OcrEngine, PDF_MIME};
use crate::domain::PageText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrMode {
    None,
    Tesseract,
    OpenAi,
}

impl OcrMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "off" => Some(Self::None),
            "tesseract" => Some(Self::Tesseract),
            "openai" => Some(Self::OpenAi),
            _ => None,
        }
    }
}

/// Everything the OCR engines are configured with (all from env, see [`OcrSettings::from_env`]).
#[derive(Clone)]
pub struct OcrSettings {
    pub mode: OcrMode,
    pub tesseract_bin: String,
    pub pdftoppm_bin: String,
    /// Tesseract languages, e.g. `vie+eng`.
    pub langs: String,
    /// Wall-clock limit of every single external process / HTTP call.
    pub timeout: Duration,
    /// Most PDF pages recognised (the rest is ignored, with a WARN).
    pub max_pages: u32,
    pub dpi: u32,
    /// Parent of the per-job temp dirs.
    pub work_dir: PathBuf,
    pub openai_model: String,
    pub openai_base_url: String,
    pub openai_api_key: Option<String>,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            mode: OcrMode::None,
            tesseract_bin: "tesseract".into(),
            pdftoppm_bin: "pdftoppm".into(),
            langs: "vie+eng".into(),
            timeout: Duration::from_secs(60),
            max_pages: 30,
            dpi: 200,
            work_dir: std::env::temp_dir(),
            openai_model: "gpt-4o-mini".into(),
            openai_base_url: "https://api.openai.com/v1".into(),
            openai_api_key: None,
        }
    }
}

impl std::fmt::Debug for OcrSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrSettings")
            .field("mode", &self.mode)
            .field("langs", &self.langs)
            .field("max_pages", &self.max_pages)
            .field("openai_api_key", &self.openai_api_key.is_some())
            .finish_non_exhaustive()
    }
}

/// Tesseract language list: letters, digits, `_` and `+` only (no option smuggling).
fn valid_langs(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
        && !s.starts_with('+')
}

impl OcrSettings {
    /// `KNOWLEDGE_OCR` (none|tesseract|openai), `KNOWLEDGE_TESSERACT_BIN`, `KNOWLEDGE_PDFTOPPM_BIN`,
    /// `KNOWLEDGE_OCR_LANGS`, `KNOWLEDGE_OCR_TIMEOUT_SECS`, `KNOWLEDGE_OCR_MAX_PAGES`,
    /// `KNOWLEDGE_OCR_DPI`, `KNOWLEDGE_OCR_OPENAI_MODEL`, `KNOWLEDGE_OCR_OPENAI_BASE_URL` and
    /// `OPENAI_API_KEY` (shared with the AI plane).
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let mut s = Self::default();
        if let Some(m) = get("KNOWLEDGE_OCR") {
            s.mode = OcrMode::parse(&m).unwrap_or_else(|| {
                tracing::warn!(value = %m, "unknown KNOWLEDGE_OCR (none|tesseract|openai); OCR disabled");
                OcrMode::None
            });
        }
        if let Some(v) = get("KNOWLEDGE_TESSERACT_BIN") {
            s.tesseract_bin = v;
        }
        if let Some(v) = get("KNOWLEDGE_PDFTOPPM_BIN") {
            s.pdftoppm_bin = v;
        }
        if let Some(v) = get("KNOWLEDGE_OCR_LANGS") {
            if valid_langs(&v) {
                s.langs = v;
            } else {
                tracing::warn!("invalid KNOWLEDGE_OCR_LANGS; using `{}`", s.langs);
            }
        }
        if let Some(n) = get("KNOWLEDGE_OCR_TIMEOUT_SECS").and_then(|v| v.parse::<u64>().ok()) {
            s.timeout = Duration::from_secs(n.max(1));
        }
        if let Some(n) = get("KNOWLEDGE_OCR_MAX_PAGES").and_then(|v| v.parse::<u32>().ok()) {
            s.max_pages = n.max(1);
        }
        if let Some(n) = get("KNOWLEDGE_OCR_DPI").and_then(|v| v.parse::<u32>().ok()) {
            s.dpi = n.clamp(72, 600);
        }
        if let Some(v) = get("KNOWLEDGE_OCR_OPENAI_MODEL") {
            s.openai_model = v;
        }
        if let Some(v) = get("KNOWLEDGE_OCR_OPENAI_BASE_URL").or_else(|| get("OPENAI_BASE_URL")) {
            s.openai_base_url = v;
        }
        s.openai_api_key = get("OPENAI_API_KEY");
        s
    }
}

/// Engine for the process environment (see [`OcrSettings::from_env`]).
pub async fn ocr_from_env() -> Arc<dyn OcrEngine> {
    build_ocr(&OcrSettings::from_env()).await
}

/// Build the configured engine; missing binaries / keys log a WARN and give [`NoopOcr`].
pub async fn build_ocr(s: &OcrSettings) -> Arc<dyn OcrEngine> {
    match s.mode {
        OcrMode::None => Arc::new(NoopOcr),
        OcrMode::Tesseract => {
            if !probe(&s.tesseract_bin, "--version").await {
                tracing::warn!(
                    bin = %s.tesseract_bin,
                    "KNOWLEDGE_OCR=tesseract but the tesseract binary is missing; OCR disabled"
                );
                return Arc::new(NoopOcr);
            }
            let rast = rasteriser(s).await;
            if rast.is_none() {
                tracing::warn!(
                    bin = %s.pdftoppm_bin,
                    "pdftoppm is missing: images are OCR'd but scanned PDFs will be skipped"
                );
            }
            tracing::info!(langs = %s.langs, "OCR engine: tesseract");
            Arc::new(TesseractOcr::new(s, rast))
        }
        OcrMode::OpenAi => {
            let Some(key) = s.openai_api_key.clone() else {
                tracing::warn!("KNOWLEDGE_OCR=openai but OPENAI_API_KEY is not set; OCR disabled");
                return Arc::new(NoopOcr);
            };
            let rast = rasteriser(s).await;
            if rast.is_none() {
                tracing::warn!(
                    "pdftoppm is missing: scanned PDFs are sent to the model as a file (unverified)"
                );
            }
            tracing::info!(model = %s.openai_model, "OCR engine: openai vision (content leaves the platform)");
            match OpenAiVisionOcr::new(s, key, rast) {
                Ok(e) => Arc::new(e),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot build the OpenAI OCR client; OCR disabled");
                    Arc::new(NoopOcr)
                }
            }
        }
    }
}

async fn rasteriser(s: &OcrSettings) -> Option<Rasteriser> {
    probe(&s.pdftoppm_bin, "-v").await.then(|| Rasteriser {
        bin: s.pdftoppm_bin.clone(),
        dpi: s.dpi,
        max_pages: s.max_pages,
        timeout: s.timeout,
        work_dir: s.work_dir.clone(),
    })
}

/// True when `bin` can be spawned (the exit status of the version flag is not checked: poppler
/// and tesseract differ in how they report it).
pub async fn probe(bin: &str, flag: &str) -> bool {
    let child = Command::new(bin)
        .arg(flag)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    match child {
        Ok(mut c) => {
            let _ = tokio::time::timeout(Duration::from_secs(10), c.wait()).await;
            true
        }
        Err(_) => false,
    }
}

// ---- process + temp dir plumbing ---------------------------------------------------------

/// Private working directory, removed when dropped (also when the future is cancelled).
struct TempDir(PathBuf);

impl TempDir {
    fn create(root: &Path) -> Result<Self, ExtractError> {
        let path = root.join(format!("warptalk-ocr-{}", Uuid::now_v7()));
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&path)
            .map_err(|e| ExtractError::Failed(format!("cannot create OCR temp dir: {e}")))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(dir = %self.0.display(), error = %e, "could not remove OCR temp dir");
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_owned(),
    }
}

/// Run `bin args...` (no shell) and return stdout. Timeout kills the child.
async fn run(bin: &str, args: &[OsString], timeout: Duration) -> Result<Vec<u8>, ExtractError> {
    let child = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ExtractError::OcrUnavailable,
            _ => ExtractError::Failed(format!("cannot start `{bin}`: {e}")),
        })?;
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => {
            return Err(ExtractError::Failed(format!(
                "`{bin}` timed out after {}s",
                timeout.as_secs()
            )));
        }
        Ok(Err(e)) => return Err(ExtractError::Failed(format!("`{bin}` failed: {e}"))),
        Ok(Ok(o)) => o,
    };
    if !out.status.success() {
        return Err(ExtractError::Failed(format!(
            "`{bin}` exited with {}: {}",
            out.status,
            truncate(&String::from_utf8_lossy(&out.stderr), 300)
        )));
    }
    Ok(out.stdout)
}

fn image_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/tiff" => "tif",
        "image/bmp" => "bmp",
        _ => "img",
    }
}

fn is_pdf(mime: &str) -> bool {
    mime == PDF_MIME || mime == "application/x-pdf"
}

/// `pdftoppm` wrapper: PDF bytes -> numbered PNG pages inside a temp dir.
#[derive(Debug, Clone)]
struct Rasteriser {
    bin: String,
    dpi: u32,
    max_pages: u32,
    timeout: Duration,
    work_dir: PathBuf,
}

struct Rasterised {
    /// Keeps the directory alive while the page files are read.
    _dir: TempDir,
    pages: Vec<(u32, PathBuf)>,
}

impl Rasteriser {
    async fn rasterise(&self, pdf: &[u8]) -> Result<Rasterised, ExtractError> {
        let dir = TempDir::create(&self.work_dir)?;
        let input = dir.path().join("in.pdf");
        tokio::fs::write(&input, pdf)
            .await
            .map_err(|e| ExtractError::Failed(format!("cannot write OCR input: {e}")))?;
        let prefix = dir.path().join("page");
        let args: Vec<OsString> = vec![
            "-r".into(),
            self.dpi.to_string().into(),
            "-png".into(),
            "-f".into(),
            "1".into(),
            "-l".into(),
            self.max_pages.to_string().into(),
            input.into_os_string(),
            prefix.into_os_string(),
        ];
        run(&self.bin, &args, self.timeout).await?;
        let mut pages = Vec::new();
        let mut rd = tokio::fs::read_dir(dir.path())
            .await
            .map_err(|e| ExtractError::Failed(format!("cannot list OCR pages: {e}")))?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name
                .strip_prefix("page-")
                .and_then(|r| r.strip_suffix(".png"))
                .and_then(|n| n.parse::<u32>().ok());
            if let Some(n) = n {
                pages.push((n, e.path()));
            }
        }
        pages.sort_by_key(|(n, _)| *n);
        if pages.len() as u32 >= self.max_pages {
            tracing::warn!(
                max = self.max_pages,
                "scanned PDF may have more pages than KNOWLEDGE_OCR_MAX_PAGES; the rest is not recognised"
            );
        }
        Ok(Rasterised { _dir: dir, pages })
    }
}

// ---- tesseract --------------------------------------------------------------------------

/// Tesseract CLI engine. At most two recognitions run at once.
pub struct TesseractOcr {
    bin: String,
    langs: String,
    timeout: Duration,
    work_dir: PathBuf,
    rasteriser: Option<Rasteriser>,
    permits: Semaphore,
}

impl TesseractOcr {
    fn new(s: &OcrSettings, rasteriser: Option<Rasteriser>) -> Self {
        Self {
            bin: s.tesseract_bin.clone(),
            langs: s.langs.clone(),
            timeout: s.timeout,
            work_dir: s.work_dir.clone(),
            rasteriser,
            permits: Semaphore::new(2),
        }
    }

    async fn ocr_file(&self, file: &Path) -> Result<String, ExtractError> {
        let args: Vec<OsString> = vec![
            file.as_os_str().to_owned(),
            "stdout".into(),
            "-l".into(),
            self.langs.clone().into(),
        ];
        let out = run(&self.bin, &args, self.timeout).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }
}

#[async_trait]
impl OcrEngine for TesseractOcr {
    async fn recognize(&self, bytes: &[u8], mime: &str) -> Result<Vec<PageText>, ExtractError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| ExtractError::Failed("OCR shut down".into()))?;
        if is_pdf(mime) {
            let Some(r) = &self.rasteriser else {
                return Err(ExtractError::OcrUnavailable);
            };
            let pages = r.rasterise(bytes).await?;
            let mut out = Vec::new();
            for (n, path) in &pages.pages {
                let text = self.ocr_file(path).await?;
                if !text.is_empty() {
                    out.push(PageText {
                        page: Some(*n),
                        text,
                    });
                }
            }
            return Ok(out);
        }
        let dir = TempDir::create(&self.work_dir)?;
        let input = dir.path().join(format!("in.{}", image_ext(mime)));
        tokio::fs::write(&input, bytes)
            .await
            .map_err(|e| ExtractError::Failed(format!("cannot write OCR input: {e}")))?;
        let text = self.ocr_file(&input).await?;
        Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![PageText { page: None, text }]
        })
    }
}

// ---- OpenAI vision ----------------------------------------------------------------------

const VISION_PROMPT: &str = "Transcribe all text in this image exactly as written, preserving \
reading order, line breaks and table structure (use markdown tables). Do not describe the image, \
do not translate, do not add commentary. If there is no readable text reply with exactly NO_TEXT.";

/// Hosted vision-model OCR (see the module docs: UNVERIFIED against the live API).
pub struct OpenAiVisionOcr {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    rasteriser: Option<Rasteriser>,
    permits: Semaphore,
}

impl OpenAiVisionOcr {
    fn new(
        s: &OcrSettings,
        api_key: String,
        rasteriser: Option<Rasteriser>,
    ) -> Result<Self, reqwest::Error> {
        Self::with_endpoint(
            &s.openai_base_url,
            api_key,
            s.openai_model.clone(),
            s.timeout,
            rasteriser,
        )
    }

    fn with_endpoint(
        base_url: &str,
        api_key: String,
        model: String,
        timeout: Duration,
        rasteriser: Option<Rasteriser>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(timeout).build()?,
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key,
            model,
            rasteriser,
            permits: Semaphore::new(2),
        })
    }

    async fn ask(&self, content: Value) -> Result<String, ExtractError> {
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 4096,
            "messages": [{ "role": "user", "content": content }],
        });
        let resp = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ExtractError::Unavailable(format!("vision request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let msg = format!("vision model answered {status}: {}", truncate(&text, 200));
            return Err(
                if status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error() {
                    ExtractError::Unavailable(msg)
                } else {
                    ExtractError::Failed(msg)
                },
            );
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ExtractError::Failed(format!("unreadable vision response: {e}")))?;
        let text = v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| ExtractError::Failed("vision response has no text".into()))?;
        let text = text.trim();
        Ok(if text == "NO_TEXT" {
            String::new()
        } else {
            text.to_owned()
        })
    }

    fn data_url(mime: &str, bytes: &[u8]) -> String {
        format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    async fn ask_image(&self, mime: &str, bytes: &[u8]) -> Result<String, ExtractError> {
        self.ask(json!([
            { "type": "text", "text": VISION_PROMPT },
            { "type": "image_url", "image_url": { "url": Self::data_url(mime, bytes) } },
        ]))
        .await
    }
}

#[async_trait]
impl OcrEngine for OpenAiVisionOcr {
    fn sends_content_externally(&self) -> bool {
        true
    }

    async fn recognize(&self, bytes: &[u8], mime: &str) -> Result<Vec<PageText>, ExtractError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| ExtractError::Failed("OCR shut down".into()))?;
        if !is_pdf(mime) {
            let text = self.ask_image(mime, bytes).await?;
            return Ok(if text.is_empty() {
                Vec::new()
            } else {
                vec![PageText { page: None, text }]
            });
        }
        let Some(r) = &self.rasteriser else {
            // No rasteriser: hand the PDF to the model as a file part (unverified).
            let text = self
                .ask(json!([
                    { "type": "text", "text": VISION_PROMPT },
                    { "type": "file", "file": {
                        "filename": "document.pdf",
                        "file_data": Self::data_url(PDF_MIME, bytes),
                    } },
                ]))
                .await?;
            return Ok(if text.is_empty() {
                Vec::new()
            } else {
                vec![PageText { page: None, text }]
            });
        };
        let pages = r.rasterise(bytes).await?;
        let mut out = Vec::new();
        for (n, path) in &pages.pages {
            let png = tokio::fs::read(path)
                .await
                .map_err(|e| ExtractError::Failed(format!("cannot read rendered page: {e}")))?;
            let text = self.ask_image("image/png", &png).await?;
            if !text.is_empty() {
                out.push(PageText {
                    page: Some(*n),
                    text,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-fake-image-";

    struct Sandbox {
        root: PathBuf,
        bin_dir: PathBuf,
        log: PathBuf,
        work: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("warptalk-ocr-test-{}", Uuid::now_v7()));
            let bin_dir = root.join("bin");
            let work = root.join("work");
            std::fs::create_dir_all(&bin_dir).unwrap();
            std::fs::create_dir_all(&work).unwrap();
            let log = root.join("calls.log");
            Self {
                root,
                bin_dir,
                log,
                work,
            }
        }

        /// Write an executable shell stub and wait until the kernel lets us run it.
        fn script(&self, name: &str, body: &str) -> String {
            let p = self.bin_dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            for _ in 0..50 {
                match std::process::Command::new(&p).arg("--stub-probe").output() {
                    Ok(_) => break,
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            p.to_string_lossy().into_owned()
        }

        fn tesseract_stub(&self) -> String {
            self.script(
                "tesseract",
                &format!(
                    "case \"$1\" in --stub-probe|-v|--version) exit 0;; esac\n\
                     printf '%s|' \"$@\" >> '{}'; echo >> '{}'\n\
                     echo \"text:$(head -c 4 \"$1\" | tr -d '\\n')\"",
                    self.log.display(),
                    self.log.display()
                ),
            )
        }

        fn pdftoppm_stub(&self, pages: u32) -> String {
            let mut writes = String::new();
            for i in 1..=pages {
                writes.push_str(&format!("echo pg{i} > \"${{last}}-{i}.png\"\n"));
            }
            self.script(
                "pdftoppm",
                &format!(
                    "case \"$1\" in --stub-probe|-v|--version) exit 0;; esac\n\
                     printf '%s|' \"$@\" >> '{}'; echo >> '{}'\n\
                     for a; do last=$a; done\n{writes}",
                    self.log.display(),
                    self.log.display()
                ),
            )
        }

        fn settings(&self, tesseract: String, pdftoppm: String) -> OcrSettings {
            OcrSettings {
                mode: OcrMode::Tesseract,
                tesseract_bin: tesseract,
                pdftoppm_bin: pdftoppm,
                work_dir: self.work.clone(),
                timeout: Duration::from_secs(5),
                ..OcrSettings::default()
            }
        }

        fn work_is_empty(&self) -> bool {
            std::fs::read_dir(&self.work).unwrap().next().is_none()
        }

        fn log(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn tesseract_image_args_and_cleanup() {
        let sb = Sandbox::new();
        let s = sb.settings(sb.tesseract_stub(), "no-such-pdftoppm-xyz".into());
        let engine = TesseractOcr::new(&s, None);
        let pages = engine.recognize(PNG, "image/png").await.unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].page, None);
        assert!(pages[0].text.starts_with("text:"), "{pages:?}");
        let log = sb.log();
        let line = log.lines().next().unwrap();
        let parts: Vec<&str> = line.trim_end_matches('|').split('|').collect();
        assert_eq!(parts.len(), 4, "{line}");
        assert!(parts[0].ends_with("/in.png"), "{line}");
        assert_eq!(&parts[1..], ["stdout", "-l", "vie+eng"]);
        assert!(sb.work_is_empty(), "temp dir must be removed");
    }

    #[tokio::test]
    async fn tesseract_scanned_pdf_uses_pdftoppm_and_numbers_pages() {
        let sb = Sandbox::new();
        let s = sb.settings(sb.tesseract_stub(), sb.pdftoppm_stub(2));
        let rast = rasteriser(&s).await;
        assert!(rast.is_some());
        let engine = TesseractOcr::new(&s, rast);
        let pages = engine
            .recognize(b"%PDF-1.4 scanned", "application/pdf")
            .await
            .unwrap();
        assert_eq!(
            pages.iter().map(|p| p.page).collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        assert_eq!(pages[1].text, "text:pg2");
        let log = sb.log();
        let first = log.lines().next().unwrap();
        // pdftoppm -r 200 -png -f 1 -l 30 <in.pdf> <prefix>
        assert!(
            first.starts_with("-r|200|-png|-f|1|-l|30|"),
            "pdftoppm args: {first}"
        );
        assert!(sb.work_is_empty());
    }

    #[tokio::test]
    async fn tesseract_timeout_kills_and_cleans_up() {
        let sb = Sandbox::new();
        let slow = sb.script(
            "tesseract",
            "case \"$1\" in --stub-probe|-v|--version) exit 0;; esac\nexec sleep 30",
        );
        let mut s = sb.settings(slow, "none".into());
        s.timeout = Duration::from_millis(300);
        let engine = TesseractOcr::new(&s, None);
        let started = std::time::Instant::now();
        let e = engine.recognize(PNG, "image/png").await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(
            matches!(&e, ExtractError::Failed(m) if m.contains("timed out")),
            "{e}"
        );
        assert!(sb.work_is_empty());
    }

    #[tokio::test]
    async fn tesseract_failure_is_reported_with_stderr() {
        let sb = Sandbox::new();
        let bad = sb.script(
            "tesseract",
            "case \"$1\" in --stub-probe|-v|--version) exit 0;; esac\necho 'Error opening data file' >&2\nexit 1",
        );
        let engine = TesseractOcr::new(&sb.settings(bad, "x".into()), None);
        let e = engine.recognize(PNG, "image/png").await.unwrap_err();
        assert!(
            matches!(&e, ExtractError::Failed(m) if m.contains("Error opening data")),
            "{e}"
        );
        assert!(sb.work_is_empty());
    }

    #[tokio::test]
    async fn pdf_without_pdftoppm_is_unavailable() {
        let sb = Sandbox::new();
        let engine = TesseractOcr::new(&sb.settings(sb.tesseract_stub(), "x".into()), None);
        let e = engine
            .recognize(b"%PDF-", "application/pdf")
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::OcrUnavailable));
    }

    #[tokio::test]
    async fn missing_binaries_fall_back_to_none() {
        let sb = Sandbox::new();
        let s = sb.settings("definitely-not-installed-tesseract".into(), "nope".into());
        let engine = build_ocr(&s).await;
        let e = engine.recognize(PNG, "image/png").await.unwrap_err();
        assert!(matches!(e, ExtractError::OcrUnavailable));
        assert!(!engine.sends_content_externally());
        // Present binaries select the real engine; pdftoppm missing only disables PDFs.
        let s = sb.settings(sb.tesseract_stub(), "nope".into());
        let engine = build_ocr(&s).await;
        assert_eq!(engine.recognize(PNG, "image/png").await.unwrap().len(), 1);
        assert!(matches!(
            engine
                .recognize(b"%PDF-", "application/pdf")
                .await
                .unwrap_err(),
            ExtractError::OcrUnavailable
        ));
        // openai without a key -> none.
        let mut s = sb.settings("x".into(), "y".into());
        s.mode = OcrMode::OpenAi;
        let engine = build_ocr(&s).await;
        assert!(matches!(
            engine.recognize(PNG, "image/png").await.unwrap_err(),
            ExtractError::OcrUnavailable
        ));
    }

    #[test]
    fn settings_from_env_lookup() {
        let m = |pairs: &'static [(&str, &str)]| {
            OcrSettings::from_lookup(move |k| {
                pairs
                    .iter()
                    .find(|(a, _)| *a == k)
                    .map(|(_, v)| (*v).to_owned())
            })
        };
        let d = m(&[]);
        assert_eq!(d.mode, OcrMode::None);
        assert_eq!(d.langs, "vie+eng");
        let s = m(&[
            ("KNOWLEDGE_OCR", "Tesseract"),
            ("KNOWLEDGE_OCR_LANGS", "eng"),
            ("KNOWLEDGE_OCR_MAX_PAGES", "5"),
            ("OPENAI_API_KEY", "sk-test"),
        ]);
        assert_eq!(s.mode, OcrMode::Tesseract);
        assert_eq!((s.langs.as_str(), s.max_pages), ("eng", 5));
        assert_eq!(s.openai_api_key.as_deref(), Some("sk-test"));
        // Bad values never escape into argument vectors.
        let s = m(&[
            ("KNOWLEDGE_OCR", "bogus"),
            ("KNOWLEDGE_OCR_LANGS", "-psm 1"),
        ]);
        assert_eq!(s.mode, OcrMode::None);
        assert_eq!(s.langs, "vie+eng");
        assert!(!format!("{s:?}").contains("sk-"));
    }

    fn vision_reply(text: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(
            json!({ "choices": [{ "message": { "role": "assistant", "content": text } }] }),
        )
    }

    fn openai(server: &MockServer, rast: Option<Rasteriser>) -> OpenAiVisionOcr {
        OpenAiVisionOcr::with_endpoint(
            &format!("{}/v1", server.uri()),
            "sk-test".into(),
            "vision-test".into(),
            Duration::from_secs(5),
            rast,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn openai_image_request_shape_and_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(vision_reply("Hóa đơn số 42"))
            .expect(1)
            .mount(&server)
            .await;
        let engine = openai(&server, None);
        assert!(engine.sends_content_externally());
        let pages = engine.recognize(PNG, "image/png").await.unwrap();
        assert_eq!(pages[0].text, "Hóa đơn số 42");
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["model"], "vision-test");
        let parts = &body["messages"][0]["content"];
        assert_eq!(parts[1]["type"], "image_url");
        assert!(
            parts[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }

    #[tokio::test]
    async fn openai_no_text_errors_and_pdf_pages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(vision_reply("NO_TEXT"))
            .mount(&server)
            .await;
        let engine = openai(&server, None);
        assert!(engine.recognize(PNG, "image/png").await.unwrap().is_empty());

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
            .mount(&server)
            .await;
        let e = openai(&server, None)
            .recognize(PNG, "image/png")
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Unavailable(_)), "{e}");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let e = openai(&server, None)
            .recognize(PNG, "image/png")
            .await
            .unwrap_err();
        assert!(matches!(e, ExtractError::Failed(_)), "{e}");

        // Scanned PDF with a rasteriser: one request per page, numbered.
        let sb = Sandbox::new();
        let s = sb.settings("x".into(), sb.pdftoppm_stub(2));
        let rast = rasteriser(&s).await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(vision_reply("page text"))
            .expect(2)
            .mount(&server)
            .await;
        let pages = openai(&server, rast)
            .recognize(b"%PDF-1.4", "application/pdf")
            .await
            .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].page, Some(2));
        assert!(sb.work_is_empty());
    }
}
