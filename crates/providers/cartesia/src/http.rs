//! REST side: voice catalog, clone creation, deletion.
//!
//! UNVERIFIED (docs host unreachable from the sandbox; shapes from the upstream SDK usage
//! `client.voices.list(is_owner=False, limit=100)`, `voices.clone(clip=, name=, language=)`,
//! `voices.delete(id)`): `GET /voices` pages as `{ "data": [..], "has_more": bool }` with
//! `starting_after=<last id>`; `POST /voices/clone` is multipart (`clip`, `name`, `language`)
//! and returns `{ "id": .. }`; `DELETE /voices/{id}`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::warn;

use crate::config::CartesiaConfig;
use crate::error::CartesiaError;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub gender: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VoicePage {
    #[serde(default)]
    data: Vec<VoiceInfo>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct CloneResponse {
    id: String,
}

/// Base language code: `"vi-VN"` -> `"vi"`.
pub fn base_language(lang: &str) -> String {
    lang.split(['-', '_'])
        .next()
        .unwrap_or(lang)
        .to_ascii_lowercase()
}

pub struct CartesiaHttp {
    client: reqwest::Client,
    base: String,
}

impl CartesiaHttp {
    pub fn new(cfg: &CartesiaConfig) -> Result<Self, CartesiaError> {
        let mut headers = HeaderMap::new();
        let mut key = HeaderValue::from_str(&cfg.api_key)
            .map_err(|_| CartesiaError::NotConfigured("api key is not a valid header value"))?;
        key.set_sensitive(true);
        headers.insert("x-api-key", key);
        headers.insert(
            "cartesia-version",
            HeaderValue::from_str(&cfg.api_version)
                .map_err(|_| CartesiaError::NotConfigured("api version"))?,
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(cfg.http_timeout_ms))
            .build()
            .map_err(|e| CartesiaError::Connect(e.to_string()))?;
        Ok(Self {
            client,
            base: cfg.http_base_url.trim_end_matches('/').to_string(),
        })
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, CartesiaError> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(CartesiaError::Api {
            status: Some(status.as_u16()),
            message: body.chars().take(300).collect(),
        })
    }

    fn map_send(e: reqwest::Error) -> CartesiaError {
        if e.is_timeout() {
            CartesiaError::Timeout("http")
        } else {
            CartesiaError::Connect(e.to_string())
        }
    }

    /// Clone a voice from a WAV clip. The returned id is a provider voice id (spec 13 section
    /// 7: not a biometric embedding held by WarpTalk).
    pub async fn clone_voice(
        &self,
        wav: &[u8],
        name: &str,
        language: &str,
    ) -> Result<String, CartesiaError> {
        let boundary = format!("wt{}", uuid::Uuid::now_v7().simple());
        let body = multipart_body(
            &boundary,
            &[("name", name), ("language", language)],
            ("clip", "clip.wav", "audio/wav", wav),
        );
        let resp = self
            .client
            .post(format!("{}/voices/clone", self.base))
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(Self::map_send)?;
        let r: CloneResponse = Self::check(resp)
            .await?
            .json()
            .await
            .map_err(|e| CartesiaError::Protocol(format!("clone response: {e}")))?;
        Ok(r.id)
    }

    pub async fn delete_voice(&self, voice_id: &str) -> Result<(), CartesiaError> {
        let resp = self
            .client
            .delete(format!("{}/voices/{voice_id}", self.base))
            .send()
            .await
            .map_err(Self::map_send)?;
        Self::check(resp).await.map(|_| ())
    }

    /// Walks the public library (the endpoint has no language filter) and buckets it by base
    /// language. `max_scanned` is a runaway guard, not a budget: keep it above the real library
    /// size or whole languages starve (upstream: `vi` sat at position 459 of ~843).
    pub async fn list_voices_by_language(
        &self,
        max_scanned: usize,
    ) -> Result<HashMap<String, Vec<VoiceInfo>>, CartesiaError> {
        let mut buckets: HashMap<String, Vec<VoiceInfo>> = HashMap::new();
        let mut scanned = 0usize;
        let mut after: Option<String> = None;
        loop {
            let mut url = format!("{}/voices?limit=100&is_owner=false", self.base);
            if let Some(a) = &after {
                url.push_str("&starting_after=");
                url.push_str(a);
            }
            let resp = self.client.get(&url).send().await.map_err(Self::map_send)?;
            let page: VoicePage = Self::check(resp)
                .await?
                .json()
                .await
                .map_err(|e| CartesiaError::Protocol(format!("voices response: {e}")))?;
            let last = page.data.last().map(|v| v.id.clone());
            for v in page.data {
                scanned += 1;
                let lang = base_language(&v.language);
                if !lang.is_empty() {
                    buckets.entry(lang).or_default().push(v);
                }
            }
            if scanned >= max_scanned {
                warn!(scanned, "cartesia voice scan capped; some language may be starved");
                break;
            }
            match (page.has_more, last) {
                (true, Some(l)) => after = Some(l),
                _ => break,
            }
        }
        Ok(buckets)
    }
}

fn multipart_body(
    boundary: &str,
    fields: &[(&str, &str)],
    file: (&str, &str, &str, &[u8]),
) -> Vec<u8> {
    let mut b = Vec::with_capacity(file.3.len() + 512);
    for (k, v) in fields {
        b.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            file.0, file.1, file.2
        )
        .as_bytes(),
    );
    b.extend_from_slice(file.3);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

/// Catalog cache: one walk of the library per `ttl`, single-flight, stale-on-error (a catalog
/// refresh must never be able to take synthesis down).
pub struct VoiceCatalog {
    http: std::sync::Arc<CartesiaHttp>,
    ttl: Duration,
    max_scanned: usize,
    state: Mutex<Option<(Instant, HashMap<String, Vec<VoiceInfo>>)>>,
}

impl VoiceCatalog {
    pub fn new(http: std::sync::Arc<CartesiaHttp>, ttl: Duration, max_scanned: usize) -> Self {
        Self {
            http,
            ttl,
            max_scanned,
            state: Mutex::new(None),
        }
    }

    /// Voices for a language (base subtag). Empty on a failed first load: callers fall back to
    /// the built-in default, never fail synthesis.
    pub async fn voices(&self, lang: &str) -> Vec<VoiceInfo> {
        let key = base_language(lang);
        let mut st = self.state.lock().await;
        let fresh = st.as_ref().is_some_and(|(at, _)| at.elapsed() < self.ttl);
        if !fresh {
            match self.http.list_voices_by_language(self.max_scanned).await {
                Ok(map) => *st = Some((Instant::now(), map)),
                Err(e) => warn!(error = %e, "cartesia catalog refresh failed"),
            }
        }
        st.as_ref()
            .and_then(|(_, m)| m.get(&key).cloned())
            .unwrap_or_default()
    }
}
