#![allow(clippy::unwrap_used)]
mod common;

use common::*;
use futures::StreamExt;
use serde_json::json;
use warptalk_llm::anthropic::AnthropicProvider;
use warptalk_llm::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

const KIND: ProviderId = ProviderId::Anthropic;

fn adapter(server: &MockServer) -> AnthropicProvider {
    AnthropicProvider::new(&cfg(KIND, &server.uri())).unwrap()
}

#[tokio::test]
async fn text_completion_and_usage_excludes_cache() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test-SECRET"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            json_resp(json!({
                "id": "msg_1", "type": "message", "role": "assistant", "stop_reason": "end_turn",
                "content": [{"type":"text","text":"Hello"}],
                "usage": {"input_tokens": 10, "cache_read_input_tokens": 200,
                          "cache_creation_input_tokens": 30, "output_tokens": 12, "some_new_field": 1}
            }))
            .insert_header("request-id", "req_011"),
        )
        .mount(&server)
        .await;
    let mut req = simple_req("claude-haiku-4-5");
    req.system = Some("sys".into());
    req.temperature = Some(0.2);
    let resp = adapter(&server).complete(&req).await.unwrap();
    assert_eq!(resp.text(), "Hello");
    assert_eq!(resp.provider_request_id.as_deref(), Some("req_011"));
    // input_tokens already excludes cache: nothing is subtracted.
    assert_eq!(
        resp.usage,
        Usage {
            input_uncached: 10,
            cache_read: 200,
            cache_write: 30,
            output: 12,
            reasoning: 0
        }
    );
    assert_eq!(resp.usage.total_input(), 240);
    let b = last_body(&server).await;
    assert_eq!(b["max_tokens"], 4096);
    assert_eq!(b["system"], "sys");
    assert_eq!(
        b["messages"][0],
        json!({"role":"user","content":[{"type":"text","text":"hi"}]})
    );
    assert!((b["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
}

#[tokio::test]
async fn tool_call_roundtrip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "stop_reason": "tool_use",
            "content": [{"type":"text","text":"Checking."},
                {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"Hanoi"}}],
            "usage": {"input_tokens": 5, "output_tokens": 3}
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "stop_reason": "end_turn",
            "content": [{"type":"text","text":"31C sunny"}],
            "usage": {"input_tokens": 9, "output_tokens": 4}
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("claude-haiku-4-5");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    assert_eq!(r1.parts[0], Part::Text("Checking.".into()));
    let Part::ToolCall { id, args_json, .. } = &r1.parts[1] else {
        panic!("no call")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(args_json).unwrap(),
        json!({"city":"Hanoi"})
    );
    let b = last_body(&server).await;
    assert_eq!(b["tools"][0]["input_schema"]["required"], json!(["city"]));

    let r2 = p
        .complete(&tool_followup("claude-haiku-4-5", r1.parts.clone(), id))
        .await
        .unwrap();
    assert_eq!(r2.text(), "31C sunny");
    let b = last_body(&server).await;
    assert_eq!(b["messages"][1]["role"], "assistant");
    assert_eq!(
        b["messages"][1]["content"][1],
        json!({"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"Hanoi"}})
    );
    assert_eq!(
        b["messages"][2],
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"31C sunny"}]})
    );
}

#[tokio::test]
async fn json_schema_body_shape_closes_objects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({"stop_reason":"end_turn","content":[]})))
        .mount(&server)
        .await;
    let mut req = simple_req("claude-haiku-4-5");
    req.response_format = Some(schema_fmt());
    adapter(&server).complete(&req).await.unwrap();
    let f = &last_body(&server).await["output_config"]["format"];
    assert_eq!(f["type"], "json_schema");
    assert_eq!(f["schema"]["additionalProperties"], false);
    assert_eq!(
        f["schema"]["properties"]["speak"],
        json!({"type":"boolean"})
    );
}

#[tokio::test]
async fn cache_control_on_system_and_stable_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({"stop_reason":"end_turn","content":[]})))
        .mount(&server)
        .await;
    let mut req = Request::new(
        "claude-haiku-4-5",
        vec![
            Message::user("doc snapshot"),
            Message::user("dynamic window"),
        ],
    );
    req.system = Some("sys".into());
    req.cache_hint = Some(CacheHint {
        stable_prefix_messages: 1,
        key: None,
    });
    adapter(&server).complete(&req).await.unwrap();
    let b = last_body(&server).await;
    assert_eq!(b["system"][0]["cache_control"], json!({"type":"ephemeral"}));
    // Both user turns merge into one message; the breakpoint sits on the first (stable) block.
    assert_eq!(
        b["messages"][0]["content"][0]["cache_control"],
        json!({"type":"ephemeral"})
    );
    assert!(
        b["messages"][0]["content"][1]
            .get("cache_control")
            .is_none()
    );
}

#[tokio::test]
async fn modern_models_drop_temperature_and_forced_tool_choice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({"stop_reason":"end_turn","content":[]})))
        .mount(&server)
        .await;
    let p = adapter(&server);
    assert!(!p.capabilities("claude-sonnet-5-5").forced_tool_choice);
    let mut req = simple_req("claude-sonnet-5-5");
    req.temperature = Some(0.0);
    req.tools = vec![weather_tool()];
    req.tool_choice = ToolChoice::Required;
    p.complete(&req).await.unwrap();
    let b = last_body(&server).await;
    assert!(b.get("temperature").is_none());
    assert_eq!(b["tool_choice"], json!({"type":"auto"}));

    req.model = "claude-haiku-4-5".into();
    req.tool_choice = ToolChoice::Named("get_weather".into());
    p.complete(&req).await.unwrap();
    assert_eq!(
        last_body(&server).await["tool_choice"],
        json!({"type":"tool","name":"get_weather"})
    );
}

#[tokio::test]
async fn streaming_thinking_tool_and_usage() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","usage":
            {"input_tokens":25,"cache_read_input_tokens":100,"cache_creation_input_tokens":0,"output_tokens":1}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"m"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_7","name":"get_weather","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"Hue\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":2}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    Mock::given(method("POST"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    let ev = collect(&adapter(&server), &simple_req("claude-haiku-4-5")).await;
    assert_eq!(
        ev,
        vec![
            StreamEvent::Opaque {
                provider: KIND,
                blob: json!({"type":"thinking","thinking":"hmm","signature":"SIG"})
            },
            StreamEvent::TextDelta("Hi".into()),
            StreamEvent::ToolCallDelta {
                index: 2,
                id: Some("toolu_7".into()),
                name: Some("get_weather".into()),
                args_delta: String::new()
            },
            StreamEvent::ToolCallDelta {
                index: 2,
                id: None,
                name: None,
                args_delta: "{\"city\":".into()
            },
            StreamEvent::ToolCallDelta {
                index: 2,
                id: None,
                name: None,
                args_delta: "\"Hue\"}".into()
            },
            StreamEvent::Usage(Usage {
                input_uncached: 25,
                cache_read: 100,
                cache_write: 0,
                output: 42,
                reasoning: 0
            }),
            StreamEvent::Done {
                stop_reason: StopReason::ToolUse
            },
        ]
    );
    assert_eq!(last_body(&server).await["stream"], true);
}

#[tokio::test]
async fn error_event_after_http_200() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"usage":{"input_tokens":1,"output_tokens":1}}}),
        ),
        (
            "error",
            json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
        ),
    ]);
    Mock::given(method("POST"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    let items: Vec<_> = adapter(&server)
        .stream(&simple_req("m"))
        .await
        .unwrap()
        .collect()
        .await;
    assert!(matches!(items.last(), Some(Err(LlmError::Overloaded(_)))));
}

#[tokio::test]
async fn thinking_blocks_are_echoed_back_unchanged_and_first() {
    let server = MockServer::start().await;
    let thinking = json!({"type":"thinking","thinking":"let me see","signature":"SIGNATURE=="});
    let redacted = json!({"type":"redacted_thinking","data":"OPAQUE"});
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "stop_reason": "tool_use",
            "content": [thinking.clone(), redacted.clone(),
                {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}],
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("claude-sonnet-5-5");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(
        r1.parts[0],
        Part::Opaque {
            provider: KIND,
            blob: thinking.clone()
        }
    );
    p.complete(&tool_followup(
        "claude-sonnet-5-5",
        r1.parts.clone(),
        "toolu_1",
    ))
    .await
    .unwrap();
    let c = &last_body(&server).await["messages"][1]["content"];
    assert_eq!(c[0], thinking);
    assert_eq!(c[1], redacted);
    assert_eq!(c[2]["type"], "tool_use");
}

#[tokio::test]
async fn validate_maps_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(wiremock::ResponseTemplate::new(401).set_body_json(
            json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}),
        ))
        .mount(&server)
        .await;
    assert!(matches!(
        adapter(&server).validate().await,
        Err(LlmError::Auth(_))
    ));
}
