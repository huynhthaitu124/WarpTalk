#![allow(clippy::unwrap_used)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use serde_json::json;
use warptalk_llm::anthropic::AnthropicProvider;
use warptalk_llm::*;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn policy() -> RetryPolicy {
    RetryPolicy {
        max_retries: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        total_deadline: Duration::from_secs(5),
    }
}

fn ok_body() -> serde_json::Value {
    json!({"stop_reason":"end_turn","content":[{"type":"text","text":"done"}]})
}

fn wrapped(server: &MockServer, p: RetryPolicy) -> Arc<dyn LlmProvider> {
    let inner = AnthropicProvider::new(&cfg(ProviderId::Anthropic, &server.uri())).unwrap();
    p.wrap(Arc::new(inner))
}

#[tokio::test]
async fn retries_429_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_json(
                    json!({"type":"error","error":{"type":"rate_limit_error","message":"slow"}}),
                ),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(ok_body()))
        .mount(&server)
        .await;
    let resp = wrapped(&server, policy())
        .complete(&simple_req("m"))
        .await
        .unwrap();
    assert_eq!(resp.text(), "done");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn does_not_retry_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"type":"error","error":{"type":"invalid_request_error","message":"bad"}}),
        ))
        .mount(&server)
        .await;
    let e = wrapped(&server, policy())
        .complete(&simple_req("m"))
        .await
        .unwrap_err();
    assert!(matches!(e, LlmError::InvalidRequest(_)));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn does_not_retry_spend_limit_429() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({"type":"error","error":
            {"type":"rate_limit_error","message":"cap","details":{"error_code":"enforced_spend_limit_reached"}}})))
        .mount(&server)
        .await;
    let e = wrapped(&server, policy())
        .complete(&simple_req("m"))
        .await
        .unwrap_err();
    assert!(matches!(e, LlmError::QuotaOrBilling(_)));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn gives_up_after_max_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(529).set_body_json(
            json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}}),
        ))
        .mount(&server)
        .await;
    let e = wrapped(&server, policy())
        .complete(&simple_req("m"))
        .await
        .unwrap_err();
    assert!(matches!(e, LlmError::Overloaded(_)));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn retry_after_beyond_deadline_gives_up_immediately() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "120")
                .set_body_json(
                    json!({"type":"error","error":{"type":"rate_limit_error","message":"slow"}}),
                ),
        )
        .mount(&server)
        .await;
    let p = RetryPolicy {
        total_deadline: Duration::from_secs(2),
        ..policy()
    };
    let e = wrapped(&server, p)
        .complete(&simple_req("m"))
        .await
        .unwrap_err();
    assert!(matches!(
        e,
        LlmError::RateLimited {
            retry_after: Some(_),
            ..
        }
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn registry_wraps_with_retry_by_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_json(json!({"type":"error","error":{"type":"api_error","message":"x"}})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(ok_body()))
        .mount(&server)
        .await;
    let mut c = cfg(ProviderId::Anthropic, &server.uri());
    c.max_retries = 2;
    let p = ProviderRegistry::build(c).unwrap();
    assert_eq!(p.id(), ProviderId::Anthropic);
    assert_eq!(p.complete(&simple_req("m")).await.unwrap().text(), "done");
}

#[test]
fn config_debug_and_serialize_never_contain_key() {
    let mut c = ProviderConfig::new(ProviderId::OpenAi, "sk-very-secret-key");
    c.default_headers
        .insert("api-key".into(), "header-secret".into());
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("sk-very-secret-key"), "{dbg}");
    assert!(!dbg.contains("header-secret"), "{dbg}");
    assert!(dbg.contains("api-key"));
    let ser = serde_json::to_string(&c).unwrap();
    assert!(!ser.contains("sk-very-secret-key"));
    assert!(!ser.contains("api_key"));
}

#[test]
fn config_deserializes_with_defaults() {
    let c: ProviderConfig = serde_json::from_value(json!({
        "kind": "openai_compatible", "api_key": "k", "base_url": "https://gw.example/v1",
        "default_headers": {"x-team": "a"}
    }))
    .unwrap();
    assert_eq!(c.kind, ProviderId::OpenAiCompatible);
    assert_eq!(c.timeout_ms, 60_000);
    assert_eq!(c.max_retries, 2);
    assert_eq!(c.default_headers["x-team"], "a");
}

#[test]
fn embedding_registry_rejects_unsupported_kinds() {
    let c = ProviderConfig::new(ProviderId::Anthropic, "k");
    assert!(ProviderRegistry::build_embedding(c).is_err());
    assert!(
        ProviderRegistry::build_embedding(ProviderConfig::new(ProviderId::Gemini, "k")).is_ok()
    );
}

#[test]
fn adapter_debug_hides_key() {
    let a = AnthropicProvider::new(&ProviderConfig::new(ProviderId::Anthropic, "sk-ant-secret"))
        .unwrap();
    assert!(!format!("{a:?}").contains("sk-ant-secret"));
}
