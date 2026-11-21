#![allow(clippy::unwrap_used)]
mod common;

use common::*;
use serde_json::json;
use warptalk_llm::openai_compatible::OpenAiCompatibleProvider;
use warptalk_llm::*;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer};

const KIND: ProviderId = ProviderId::OpenAiCompatible;

fn adapter(server: &MockServer) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(&cfg(KIND, &server.uri())).unwrap()
}

#[tokio::test]
async fn text_completion_and_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-test-SECRET"))
        .respond_with(
            json_resp(json!({
                "id":"chatcmpl-1",
                "choices":[{"index":0,"message":{"role":"assistant","content":"Hello"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":50,"completion_tokens":9,
                         "prompt_tokens_details":{"cached_tokens":20},
                         "completion_tokens_details":{"reasoning_tokens":3}}
            }))
            .insert_header("x-request-id", "rq1"),
        )
        .mount(&server)
        .await;
    let mut req = simple_req("gpt-4o");
    req.system = Some("sys".into());
    req.max_output_tokens = Some(32);
    let resp = adapter(&server).complete(&req).await.unwrap();
    assert_eq!(resp.text(), "Hello");
    assert_eq!(resp.provider_request_id.as_deref(), Some("rq1"));
    assert_eq!(
        resp.usage,
        Usage {
            input_uncached: 30,
            cache_read: 20,
            cache_write: 0,
            output: 9,
            reasoning: 3
        }
    );
    let b = last_body(&server).await;
    assert_eq!(b["messages"][0], json!({"role":"system","content":"sys"}));
    assert_eq!(b["messages"][1], json!({"role":"user","content":"hi"}));
    assert_eq!(b["max_tokens"], 32);
    assert_eq!(b["stream"], false);
}

#[tokio::test]
async fn tool_call_roundtrip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Hanoi\"}"}}]},
                "finish_reason":"tool_calls"}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "choices":[{"message":{"role":"assistant","content":"31C"},"finish_reason":"stop"}]
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("gpt-4o");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    let b = last_body(&server).await;
    assert_eq!(b["tools"][0]["function"]["name"], "get_weather");
    assert_eq!(
        b["tools"][0]["function"]["parameters"]["required"],
        json!(["city"])
    );

    let r2 = p
        .complete(&tool_followup("gpt-4o", r1.parts.clone(), "call_1"))
        .await
        .unwrap();
    assert_eq!(r2.text(), "31C");
    let b = last_body(&server).await;
    assert_eq!(b["messages"][1]["role"], "assistant");
    assert!(b["messages"][1]["content"].is_null());
    assert_eq!(
        b["messages"][1]["tool_calls"][0],
        json!({"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Hanoi\"}"}})
    );
    assert_eq!(
        b["messages"][2],
        json!({"role":"tool","tool_call_id":"call_1","content":"31C sunny"})
    );
}

#[tokio::test]
async fn json_schema_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(
            json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}]}),
        ))
        .mount(&server)
        .await;
    let mut req = simple_req("gpt-4o");
    req.response_format = Some(schema_fmt());
    req.tools = vec![weather_tool()];
    req.tool_choice = ToolChoice::Required;
    adapter(&server).complete(&req).await.unwrap();
    let b = last_body(&server).await;
    assert_eq!(b["response_format"]["type"], "json_schema");
    assert_eq!(b["response_format"]["json_schema"]["name"], "decision");
    assert_eq!(b["response_format"]["json_schema"]["strict"], true);
    assert_eq!(
        b["response_format"]["json_schema"]["schema"]["required"],
        json!(["speak"])
    );
    assert_eq!(b["tool_choice"], "required");
}

#[tokio::test]
async fn streaming_text_tool_and_usage_after_finish() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "",
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"He"}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"content":"llo"}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_5","type":"function","function":{"name":"get_weather","arguments":""}}]}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":\"Hue\"}"}}]}}]}),
        ),
        (
            "",
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ),
        (
            "",
            json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":2}}}),
        ),
    ]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    let ev = collect(&adapter(&server), &simple_req("gpt-4o")).await;
    assert_eq!(
        ev,
        vec![
            StreamEvent::TextDelta("He".into()),
            StreamEvent::TextDelta("llo".into()),
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_5".into()),
                name: Some("get_weather".into()),
                args_delta: String::new()
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "{\"city\":\"Hue\"}".into()
            },
            StreamEvent::Usage(Usage {
                input_uncached: 10,
                cache_read: 2,
                cache_write: 0,
                output: 4,
                reasoning: 0
            }),
            StreamEvent::Done {
                stop_reason: StopReason::ToolUse
            },
        ]
    );
    let b = last_body(&server).await;
    assert_eq!(b["stream"], true);
    assert_eq!(b["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn reasoning_content_is_echoed_back_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "choices":[{"message":{"role":"assistant","reasoning_content":"thought it through",
                "content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"get_weather","arguments":"{}"}}]},
                "finish_reason":"tool_calls"}]
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("deepseek-reasoner");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert!(matches!(&r1.parts[0], Part::Opaque { provider: KIND, .. }));
    p.complete(&tool_followup("deepseek-reasoner", r1.parts.clone(), "c1"))
        .await
        .unwrap();
    let b = last_body(&server).await;
    assert_eq!(b["messages"][1]["reasoning_content"], "thought it through");
}

#[tokio::test]
async fn azure_style_base_url_query_and_custom_auth_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .and(header("api-key", "azure-secret"))
        .respond_with(json_resp(
            json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}),
        ))
        .mount(&server)
        .await;
    let mut c = ProviderConfig::new(KIND, "");
    c.base_url = Some(format!(
        "{}/openai/deployments/gpt4o?api-version=2024-10-21",
        server.uri()
    ));
    c.default_headers
        .insert("api-key".into(), "azure-secret".into());
    c.max_retries = 0;
    let p = ProviderRegistry::build(c).unwrap();
    assert_eq!(p.complete(&simple_req("gpt4o")).await.unwrap().text(), "ok");
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("authorization").is_none());
    let _ = header_exists("x");
}

#[tokio::test]
async fn invalid_base_url_is_rejected() {
    let mut c = ProviderConfig::new(KIND, "k");
    c.base_url = Some("not a url".into());
    assert!(matches!(
        ProviderRegistry::build(c),
        Err(LlmError::InvalidRequest(_))
    ));
}
