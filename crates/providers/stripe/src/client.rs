use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::StripeError;
use crate::model::{BillingPortalSession, CheckoutRequest, CheckoutSession, Customer, Subscription};

const DEFAULT_BASE_URL: &str = "https://api.stripe.com";

#[derive(Clone)]
pub struct StripeConfig {
    secret_key: String,
    /// Override for tests (wiremock); default `https://api.stripe.com`.
    pub base_url: String,
    pub timeout: Duration,
}

impl StripeConfig {
    pub fn new(secret_key: impl Into<String>) -> Self {
        Self {
            secret_key: secret_key.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            timeout: Duration::from_secs(20),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into().trim_end_matches('/').to_owned();
        self
    }
}

impl std::fmt::Debug for StripeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeConfig")
            .field("secret_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Minimal Stripe REST client.
#[derive(Clone)]
pub struct StripeClient {
    http: reqwest::Client,
    cfg: StripeConfig,
}

impl std::fmt::Debug for StripeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeClient").field("cfg", &self.cfg).finish()
    }
}

impl StripeClient {
    pub fn new(cfg: StripeConfig) -> Result<Self, StripeError> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| StripeError::Transport(e.to_string()))?;
        Ok(Self { http, cfg })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.cfg.base_url)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, StripeError> {
        let resp = req
            .bearer_auth(&self.cfg.secret_key)
            .send()
            .await
            .map_err(|e| StripeError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| StripeError::Transport(e.without_url().to_string()))?;
        if !status.is_success() {
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let err = v.get("error");
            let field = |k: &str| {
                err.and_then(|e| e.get(k))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            return Err(StripeError::Api {
                status: status.as_u16(),
                code: field("code"),
                message: field("message").unwrap_or_else(|| "unspecified error".to_owned()),
            });
        }
        serde_json::from_str(&body).map_err(|e| StripeError::Decode(e.to_string()))
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<T, StripeError> {
        let mut req = self.http.post(self.url(path)).form(form);
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        self.send(req).await
    }

    /// `POST /v1/checkout/sessions`. `idempotency_key` makes a retry return the same session.
    pub async fn create_checkout_session(
        &self,
        req: &CheckoutRequest,
        idempotency_key: Option<&str>,
    ) -> Result<CheckoutSession, StripeError> {
        self.post("/v1/checkout/sessions", &req.form(), idempotency_key)
            .await
    }

    /// `POST /v1/customers`.
    pub async fn create_customer(
        &self,
        email: Option<&str>,
        name: Option<&str>,
        metadata: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<Customer, StripeError> {
        let mut f: Vec<(String, String)> = Vec::new();
        if let Some(e) = email {
            f.push(("email".into(), e.into()));
        }
        if let Some(n) = name {
            f.push(("name".into(), n.into()));
        }
        for (k, v) in metadata {
            f.push((format!("metadata[{k}]"), v.clone()));
        }
        self.post("/v1/customers", &f, idempotency_key).await
    }

    /// `GET /v1/customers/{id}`.
    pub async fn retrieve_customer(&self, id: &str) -> Result<Customer, StripeError> {
        self.send(self.http.get(self.url(&format!("/v1/customers/{}", enc(id)))))
            .await
    }

    /// `POST /v1/billing_portal/sessions`. UNVERIFIED: needs a portal configuration saved in the
    /// Stripe dashboard (otherwise Stripe answers 400).
    pub async fn create_portal_session(
        &self,
        customer: &str,
        return_url: &str,
    ) -> Result<BillingPortalSession, StripeError> {
        let f = vec![
            ("customer".to_owned(), customer.to_owned()),
            ("return_url".to_owned(), return_url.to_owned()),
        ];
        self.post("/v1/billing_portal/sessions", &f, None).await
    }

    /// `GET /v1/subscriptions/{id}`.
    pub async fn retrieve_subscription(&self, id: &str) -> Result<Subscription, StripeError> {
        self.send(
            self.http
                .get(self.url(&format!("/v1/subscriptions/{}", enc(id)))),
        )
        .await
    }
}

/// Percent-encode a path segment (ids are `[A-Za-z0-9_]`, but never trust input).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{CheckoutLine, CheckoutMode};
    use wiremock::matchers::{body_string_contains, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> StripeClient {
        StripeClient::new(StripeConfig::new("sk_test_x").with_base_url(server.uri())).unwrap()
    }

    #[tokio::test]
    async fn checkout_session_is_form_encoded_with_bearer_and_idempotency_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/checkout/sessions"))
            .and(header("authorization", "Bearer sk_test_x"))
            .and(header("idempotency-key", "k1"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("mode=subscription"))
            .and(body_string_contains("line_items%5B0%5D%5Bprice%5D=price_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"id": "cs_1", "url": "https://checkout.stripe.test/c/cs_1"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let s = client(&server)
            .create_checkout_session(
                &CheckoutRequest {
                    mode: CheckoutMode::Subscription,
                    line: CheckoutLine::Price("price_1".into()),
                    success_url: "https://a/ok".into(),
                    cancel_url: "https://a/no".into(),
                    customer: None,
                    client_reference_id: None,
                    metadata: vec![],
                },
                Some("k1"),
            )
            .await
            .unwrap();
        assert_eq!(s.id, "cs_1");
        assert_eq!(s.url.as_deref(), Some("https://checkout.stripe.test/c/cs_1"));
    }

    #[tokio::test]
    async fn api_errors_are_typed_and_carry_no_secret() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/subscriptions/sub_x"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": {"type": "invalid_request_error", "code": "resource_missing", "message": "No such subscription"}
            })))
            .mount(&server)
            .await;
        let e = client(&server).retrieve_subscription("sub_x").await.unwrap_err();
        match &e {
            StripeError::Api { status, code, .. } => {
                assert_eq!(*status, 404);
                assert_eq!(code.as_deref(), Some("resource_missing"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!e.is_retryable());
        assert!(!format!("{e} {:?}", client(&server)).contains("sk_test_x"));
    }

    #[tokio::test]
    async fn customer_portal_and_subscription() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/customers"))
            .and(body_string_contains("metadata%5Bworkspace_id%5D=w1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "cus_1"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/customers/cus_1"))
            .and(header_exists("authorization"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "cus_1", "email": "a@b.c"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/billing_portal/sessions"))
            .and(body_string_contains("customer=cus_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"id": "bps_1", "url": "https://portal.stripe.test/p"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/subscriptions/sub_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "sub_1", "status": "active", "customer": "cus_1",
                "current_period_start": 100, "current_period_end": 200,
                "items": {"data": [{"price": {"id": "price_9"}}]}
            })))
            .mount(&server)
            .await;
        let c = client(&server);
        let cus = c
            .create_customer(None, None, &[("workspace_id".into(), "w1".into())], None)
            .await
            .unwrap();
        assert_eq!(cus.id, "cus_1");
        assert_eq!(c.retrieve_customer("cus_1").await.unwrap().email.as_deref(), Some("a@b.c"));
        assert_eq!(
            c.create_portal_session("cus_1", "https://a/back").await.unwrap().url,
            "https://portal.stripe.test/p"
        );
        let sub = c.retrieve_subscription("sub_1").await.unwrap();
        assert_eq!(sub.period(), Some((100, 200)));
        assert_eq!(sub.price_id(), Some("price_9"));
    }
}
