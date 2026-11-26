//! Email delivery: [`EmailSender`] port, the Resend HTTPS adapter, a log-only dev adapter and
//! the product mail templates (`templates`).

pub mod templates;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A message to deliver. `tags` are provider analytics tags (names/values: ASCII letters,
/// digits, `_` or `-`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub tags: Vec<(String, String)>,
}

/// Provider message id (stored on invitations for support lookups).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageId(pub String);

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// 4xx other than 429: the provider refused the message; retrying will not help.
    #[error("email rejected by provider (status {status}): {message}")]
    Rejected { status: u16, message: String },
    /// 429: back off and retry.
    #[error("email provider rate limited")]
    RateLimited { retry_after_secs: Option<u64> },
    /// 5xx, timeouts, connection errors: retryable.
    #[error("email provider unavailable: {0}")]
    Unavailable(String),
    #[error("email configuration error: {0}")]
    Config(String),
}

impl EmailError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Unavailable(_))
    }
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send(&self, email: Email) -> Result<MessageId, EmailError>;
}

/// Development sender: logs recipient and subject only (never bodies: they contain tokens).
#[derive(Debug, Default, Clone, Copy)]
pub struct LogSender;

#[async_trait]
impl EmailSender for LogSender {
    async fn send(&self, email: Email) -> Result<MessageId, EmailError> {
        tracing::info!(
            to = %email.to,
            subject = %email.subject,
            "email not delivered (LogSender; no provider configured)"
        );
        Ok(MessageId("log".to_owned()))
    }
}

const RESEND_BASE_URL: &str = "https://api.resend.com";

/// Resend HTTPS API sender (`POST {base}/emails`, bearer auth).
///
/// // UNVERIFIED: request/response shape is from memory of the public Resend docs
/// (`{from, to:[..], subject, html, text, tags:[{name,value}]}` -> `{"id": "..."}`) and was not
/// checked against the live API from this environment; only against a wiremock stub.
pub struct ResendSender {
    http: reqwest::Client,
    api_key: String,
    from: String,
    base_url: String,
}

impl std::fmt::Debug for ResendSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResendSender")
            .field("from", &self.from)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize)]
struct ResendTag<'a> {
    name: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct ResendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    html: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<ResendTag<'a>>,
}

#[derive(Deserialize)]
struct ResendOk {
    id: String,
}

#[derive(Deserialize)]
struct ResendErr {
    message: Option<String>,
}

impl ResendSender {
    /// `from` is the verified sender, e.g. `WarpTalk <no-reply@example.com>`.
    pub fn new(api_key: impl Into<String>, from: impl Into<String>) -> Result<Self, EmailError> {
        let api_key = api_key.into();
        let from = from.into();
        if api_key.trim().is_empty() {
            return Err(EmailError::Config("RESEND_API_KEY is empty".to_owned()));
        }
        if from.trim().is_empty() {
            return Err(EmailError::Config("EMAIL_FROM is empty".to_owned()));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| EmailError::Config(format!("http client: {e}")))?;
        Ok(Self {
            http,
            api_key,
            from,
            base_url: RESEND_BASE_URL.to_owned(),
        })
    }

    /// Points at another API origin (tests, gateways).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }
}

#[async_trait]
impl EmailSender for ResendSender {
    async fn send(&self, email: Email) -> Result<MessageId, EmailError> {
        let body = ResendRequest {
            from: &self.from,
            to: [&email.to],
            subject: &email.subject,
            html: &email.html,
            text: &email.text,
            tags: email
                .tags
                .iter()
                .map(|(n, v)| ResendTag { name: n, value: v })
                .collect(),
        };
        let resp = self
            .http
            .post(format!("{}/emails", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            // reqwest errors can embed the URL but never the Authorization header.
            .map_err(|e| EmailError::Unavailable(e.without_url().to_string()))?;
        let status = resp.status();
        if status.is_success() {
            let ok: ResendOk = resp
                .json()
                .await
                .map_err(|e| EmailError::Unavailable(format!("unreadable provider reply: {e}")))?;
            return Ok(MessageId(ok.id));
        }
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let message = resp
            .json::<ResendErr>()
            .await
            .ok()
            .and_then(|e| e.message)
            .unwrap_or_default();
        let code = status.as_u16();
        Err(if code == 429 {
            EmailError::RateLimited {
                retry_after_secs: retry_after,
            }
        } else if status.is_client_error() {
            EmailError::Rejected {
                status: code,
                message,
            }
        } else {
            EmailError::Unavailable(format!("provider status {code}: {message}"))
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mail() -> Email {
        Email {
            to: "a@example.test".into(),
            subject: "Hi".into(),
            html: "<p>x</p>".into(),
            text: "x".into(),
            tags: vec![("kind".into(), "verify".into())],
        }
    }

    async fn sender(server: &MockServer) -> ResendSender {
        ResendSender::new("re_test_key", "WarpTalk <no-reply@example.test>")
            .unwrap()
            .with_base_url(server.uri())
    }

    #[tokio::test]
    async fn sends_and_returns_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails"))
            .and(header("authorization", "Bearer re_test_key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;
        let id = sender(&server).await.send(mail()).await.unwrap();
        assert_eq!(id, MessageId("m1".into()));
        let req = &server.received_requests().await.unwrap()[0];
        let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(v["to"][0], "a@example.test");
        assert_eq!(v["from"], "WarpTalk <no-reply@example.test>");
        assert_eq!(v["tags"][0]["name"], "kind");
    }

    #[tokio::test]
    async fn classifies_errors() {
        for (status, retryable) in [(422u16, false), (401, false), (429, true), (500, true)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("retry-after", "7")
                        .set_body_json(serde_json::json!({"message": "nope"})),
                )
                .mount(&server)
                .await;
            let err = sender(&server).await.send(mail()).await.unwrap_err();
            assert_eq!(err.is_retryable(), retryable, "status {status}: {err}");
            match (status, &err) {
                (429, EmailError::RateLimited { retry_after_secs }) => {
                    assert_eq!(*retry_after_secs, Some(7))
                }
                (422 | 401, EmailError::Rejected { status: s, message }) => {
                    assert_eq!(*s, status);
                    assert_eq!(message, "nope");
                }
                (500, EmailError::Unavailable(_)) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn api_key_never_in_debug_or_errors() {
        let s = ResendSender::new("re_secret_value", "x <x@example.test>").unwrap();
        assert!(!format!("{s:?}").contains("re_secret_value"));
        let dead = ResendSender::new("re_secret_value", "x <x@example.test>")
            .unwrap()
            .with_base_url("http://127.0.0.1:1");
        let err = dead.send(mail()).await.unwrap_err();
        assert!(!err.to_string().contains("re_secret_value"));
        assert!(err.is_retryable());
    }

    #[test]
    fn empty_config_rejected() {
        assert!(ResendSender::new(" ", "a@b.c").is_err());
        assert!(ResendSender::new("k", "").is_err());
    }
}
