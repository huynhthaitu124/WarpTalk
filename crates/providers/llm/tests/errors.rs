#![allow(clippy::unwrap_used)]
mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use warptalk_llm::*;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const ALL: [ProviderId; 4] = [
    ProviderId::OpenAi,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenAiCompatible,
];

async fn err_for(kind: ProviderId, status: u16, headers: &[(&str, &str)], body: Value) -> LlmError {
    let server = MockServer::start().await;
    let mut t = ResponseTemplate::new(status).set_body_json(body);
    for (k, v) in headers {
        t = t.insert_header(*k, *v);
    }
    Mock::given(method("POST"))
        .respond_with(t)
        .mount(&server)
        .await;
    provider(kind, &server)
        .complete(&simple_req("m"))
        .await
        .unwrap_err()
}

/// Native error body shape of each provider.
fn body(kind: ProviderId, ty: &str, code: &str, msg: &str) -> Value {
    match kind {
        ProviderId::Anthropic => json!({"type":"error","error":{"type":ty,"message":msg}}),
        ProviderId::Gemini => json!({"error":{"code":0,"message":msg,"status":ty}}),
        _ => json!({"error":{"type":ty,"code":code,"message":msg}}),
    }
}

#[tokio::test]
async fn http_429_with_retry_after_is_rate_limited() {
    for k in ALL {
        let e = err_for(
            k,
            429,
            &[("retry-after", "7")],
            body(k, "rate_limit_error", "rate_limit_exceeded", "slow down"),
        )
        .await;
        match &e {
            LlmError::RateLimited { retry_after, .. } => {
                assert_eq!(*retry_after, Some(Duration::from_secs(7)), "{k}")
            }
            other => panic!("{k}: {other:?}"),
        }
        assert!(e.is_retryable());
    }
}

#[tokio::test]
async fn spend_limit_429_is_quota_not_rate_limit() {
    let cases = [
        (
            ProviderId::OpenAi,
            json!({"error":{"type":"insufficient_quota","code":"organization_spend_limit_exceeded","message":"limit"}}),
        ),
        (
            ProviderId::OpenAiCompatible,
            json!({"error":{"type":"x","code":"credit_balance_exhausted","message":"no credit"}}),
        ),
        (
            ProviderId::Anthropic,
            json!({"type":"error","error":{"type":"rate_limit_error","message":"cap","details":{"error_code":"enforced_spend_limit_reached"}}}),
        ),
    ];
    for (k, b) in cases {
        let e = err_for(k, 429, &[], b).await;
        assert!(matches!(e, LlmError::QuotaOrBilling(_)), "{k}: {e:?}");
        assert!(!e.is_retryable());
    }
    let e = err_for(
        ProviderId::Anthropic,
        402,
        &[],
        body(ProviderId::Anthropic, "billing_error", "", "pay up"),
    )
    .await;
    assert!(matches!(e, LlmError::QuotaOrBilling(_)));
}

#[tokio::test]
async fn overloaded_529_and_503() {
    let e = err_for(
        ProviderId::Anthropic,
        529,
        &[],
        body(ProviderId::Anthropic, "overloaded_error", "", "Overloaded"),
    )
    .await;
    assert!(matches!(e, LlmError::Overloaded(_)) && e.is_retryable());
    for k in [
        ProviderId::OpenAi,
        ProviderId::Gemini,
        ProviderId::OpenAiCompatible,
    ] {
        let e = err_for(k, 503, &[], body(k, "UNAVAILABLE", "", "try later")).await;
        assert!(
            matches!(e, LlmError::Overloaded(_)) && e.is_retryable(),
            "{k}"
        );
    }
}

#[tokio::test]
async fn unauthorized_401_and_gemini_invalid_key() {
    for k in ALL {
        let e = err_for(
            k,
            401,
            &[],
            body(k, "authentication_error", "invalid_api_key", "bad key"),
        )
        .await;
        assert!(matches!(e, LlmError::Auth(_)), "{k}");
        assert!(!e.is_retryable());
    }
    let e = err_for(
        ProviderId::Gemini,
        400,
        &[],
        json!({"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT",
                "details":[{"reason":"API_KEY_INVALID"}]}}),
    )
    .await;
    assert!(matches!(e, LlmError::Auth(_)));
}

#[tokio::test]
async fn context_length_400() {
    let cases = [
        (
            ProviderId::Anthropic,
            body(
                ProviderId::Anthropic,
                "invalid_request_error",
                "",
                "prompt is too long: 250000 tokens > 200000 maximum",
            ),
        ),
        (
            ProviderId::OpenAi,
            json!({"error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"too big"}}),
        ),
        (
            ProviderId::OpenAiCompatible,
            json!({"error":{"message":"This model's maximum context length is 8192 tokens"}}),
        ),
        (
            ProviderId::Gemini,
            json!({"error":{"code":400,"status":"INVALID_ARGUMENT","message":"The input token count (2000000) exceeds the maximum number of tokens allowed (1048576)."}}),
        ),
    ];
    for (k, b) in cases {
        let e = err_for(k, 400, &[], b).await;
        assert!(matches!(e, LlmError::ContextTooLong(_)), "{k}: {e:?}");
        assert!(!e.is_retryable());
    }
}

#[tokio::test]
async fn plain_400_is_invalid_request_and_content_filter() {
    let e = err_for(
        ProviderId::Anthropic,
        400,
        &[],
        body(
            ProviderId::Anthropic,
            "invalid_request_error",
            "",
            "tool_choice any not supported",
        ),
    )
    .await;
    assert!(matches!(e, LlmError::InvalidRequest(_)));
    let e = err_for(
        ProviderId::OpenAiCompatible,
        400,
        &[],
        json!({"error":{"code":"content_filter","message":"blocked by policy"}}),
    )
    .await;
    assert!(matches!(e, LlmError::ContentFiltered(_)));
}

#[tokio::test]
async fn provider_message_never_contains_the_key() {
    let e = err_for(
        ProviderId::OpenAi,
        401,
        &[],
        json!({"error":{"message":"Incorrect API key provided: sk-test-SECRET.","code":"invalid_api_key"}}),
    )
    .await;
    assert!(!format!("{e} {e:?}").contains("SECRET"));
}

#[tokio::test]
async fn timeout_decorator_maps_slow_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(400))
                .set_body_json(json!({"status":"completed","output":[]})),
        )
        .mount(&server)
        .await;
    let mut c = cfg(ProviderId::OpenAi, &server.uri());
    c.timeout_ms = 50;
    let p = ProviderRegistry::build(c).unwrap();
    let e = p.complete(&simple_req("m")).await.unwrap_err();
    assert!(matches!(e, LlmError::Timeout), "{e:?}");
}
