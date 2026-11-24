//! OpenAI ephemeral token for a streaming transcription session.
//!
//! The platform key stays on the server; the app receives a short-lived client secret and opens
//! the realtime WebSocket itself. Everything provider specific is isolated in this file.
//!
//! UNVERIFIED: the endpoint, the request body and the response shape below follow OpenAI's
//! Realtime "client secrets" API for a transcription session as documented at the time of
//! writing; they were not exercised against the live API from this environment. Adjust
//! `ENDPOINT_PATH`, `request_body` and `parse_response` once verified with a real key.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::application::ports::{SttIssueError, SttToken, SttTokenIssuer, SttTokenRequest};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";
pub const DEFAULT_MODEL: &str = "gpt-4o-transcribe";
/// UNVERIFIED endpoint path (relative to the base URL).
const ENDPOINT_PATH: &str = "/v1/realtime/client_secrets";
/// How long the minted secret may be used to open the connection.
const SECRET_TTL_SECS: i64 = 600;

#[derive(Clone)]
pub struct OpenAiSttConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl std::fmt::Debug for OpenAiSttConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiSttConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

impl OpenAiSttConfig {
    /// `OPENAI_API_KEY` (required, non-empty), `OPENAI_BASE_URL`, `COPILOT_STT_MODEL`.
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            api_key: get("OPENAI_API_KEY")?,
            base_url: get("OPENAI_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            model: get("COPILOT_STT_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
        })
    }
}

pub struct OpenAiSttTokenIssuer {
    cfg: OpenAiSttConfig,
    http: reqwest::Client,
}

impl OpenAiSttTokenIssuer {
    pub fn new(cfg: OpenAiSttConfig) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { cfg, http })
    }
}

/// ISO-639-1 hint from a language tag (`vi-VN` -> `vi`); `None` when it is not two letters.
fn language_hint(tag: &str) -> Option<String> {
    let primary = tag.split('-').next()?.to_ascii_lowercase();
    (primary.len() == 2 && primary.bytes().all(|b| b.is_ascii_lowercase())).then_some(primary)
}

/// UNVERIFIED request body.
fn request_body(model: &str, language: Option<&str>) -> (Value, Value) {
    let mut transcription = json!({ "model": model });
    if let Some(l) = language.and_then(language_hint) {
        transcription["language"] = json!(l);
    }
    let session = json!({
        "type": "transcription",
        "audio": { "input": {
            "format": { "type": "audio/pcm", "rate": 24000 },
            "transcription": transcription,
            "turn_detection": { "type": "server_vad" }
        }}
    });
    let body = json!({
        "expires_after": { "anchor": "created_at", "seconds": SECRET_TTL_SECS },
        "session": session
    });
    (body, session)
}

/// UNVERIFIED response shape: `{ "value", "expires_at" }` (also accepts the older nested
/// `{ "client_secret": { "value", "expires_at" } }`). `expires_at` is epoch seconds.
fn parse_response(v: &Value, now: DateTime<Utc>) -> Option<(String, DateTime<Utc>)> {
    let node = v.get("client_secret").unwrap_or(v);
    let token = node.get("value")?.as_str()?.to_owned();
    if token.is_empty() {
        return None;
    }
    let expires = node
        .get("expires_at")
        .and_then(Value::as_i64)
        .and_then(|s| DateTime::from_timestamp(s, 0))
        .unwrap_or_else(|| now + chrono::Duration::seconds(SECRET_TTL_SECS));
    Some((token, expires))
}

fn websocket_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    format!("{ws}/v1/realtime?intent=transcription")
}

#[async_trait]
impl SttTokenIssuer for OpenAiSttTokenIssuer {
    async fn issue(&self, req: SttTokenRequest) -> Result<SttToken, SttIssueError> {
        let (body, session) = request_body(&self.cfg.model, req.language.as_deref());
        let url = format!("{}{ENDPOINT_PATH}", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .json(&body)
            .send()
            .await
            // The transport error text never contains the key; still keep it terse.
            .map_err(|e| SttIssueError::Upstream(format!("request failed: {}", e.without_url())))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SttIssueError::Upstream(format!(
                "provider answered {status}"
            )));
        }
        let json: Value = resp
            .json()
            .await
            .map_err(|_| SttIssueError::Upstream("unreadable provider response".into()))?;
        let (token, expires_at) = parse_response(&json, Utc::now()).ok_or_else(|| {
            SttIssueError::Upstream("provider response has no client secret".into())
        })?;
        Ok(SttToken {
            provider: "openai".into(),
            model: self.cfg.model.clone(),
            token,
            expires_at,
            websocket_url: websocket_url(&self.cfg.base_url),
            session_config: session,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use warptalk_kernel::RoomId;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn issuer(server: &MockServer) -> OpenAiSttTokenIssuer {
        OpenAiSttTokenIssuer::new(OpenAiSttConfig {
            api_key: "sk-platform".into(),
            base_url: server.uri(),
            model: "gpt-4o-transcribe".into(),
        })
        .unwrap()
    }

    fn req(lang: &str) -> SttTokenRequest {
        SttTokenRequest {
            session: RoomId::new(),
            language: Some(lang.into()),
        }
    }

    #[tokio::test]
    async fn mints_a_client_secret() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/realtime/client_secrets"))
            .and(header("authorization", "Bearer sk-platform"))
            .and(body_partial_json(json!({
                "session": { "type": "transcription",
                    "audio": { "input": { "transcription": { "model": "gpt-4o-transcribe", "language": "vi" } } } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": "ek_test_123", "expires_at": 1_900_000_000
            })))
            .expect(1)
            .mount(&server)
            .await;
        let t = issuer(&server).issue(req("vi-VN")).await.unwrap();
        assert_eq!(t.token, "ek_test_123");
        assert_eq!(t.provider, "openai");
        assert_eq!(t.expires_at.timestamp(), 1_900_000_000);
        assert!(
            t.websocket_url
                .ends_with("/v1/realtime?intent=transcription")
        );
        assert!(t.websocket_url.starts_with("ws://"));
        assert_eq!(t.session_config["type"], "transcription");
        assert!(
            !format!("{t:?}").contains("ek_test_123"),
            "Debug must redact the token"
        );
    }

    #[tokio::test]
    async fn accepts_the_nested_response_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "client_secret": { "value": "ek_nested", "expires_at": 1_900_000_001 }
            })))
            .mount(&server)
            .await;
        let t = issuer(&server).issue(req("en")).await.unwrap();
        assert_eq!(t.token, "ek_nested");
    }

    #[tokio::test]
    async fn provider_errors_are_upstream_errors_without_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key sk-platform"))
            .mount(&server)
            .await;
        let e = issuer(&server).issue(req("en")).await.unwrap_err();
        match e {
            SttIssueError::Upstream(m) => {
                assert!(m.contains("401"));
                assert!(!m.contains("sk-platform"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_response_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "nope": 1 })))
            .mount(&server)
            .await;
        assert!(issuer(&server).issue(req("en")).await.is_err());
    }

    #[test]
    fn language_hints() {
        assert_eq!(language_hint("vi-VN").as_deref(), Some("vi"));
        assert_eq!(language_hint("EN").as_deref(), Some("en"));
        assert_eq!(language_hint("fil"), None);
    }
}
