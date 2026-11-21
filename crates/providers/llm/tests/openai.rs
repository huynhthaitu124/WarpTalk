#![allow(clippy::unwrap_used)]
mod common;

use common::*;
use serde_json::json;
use warptalk_llm::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

const KIND: ProviderId = ProviderId::OpenAi;

fn adapter(server: &MockServer) -> warptalk_llm::openai::OpenAiProvider {
    warptalk_llm::openai::OpenAiProvider::new(&cfg(KIND, &server.uri())).unwrap()
}

#[tokio::test]
async fn text_completion_usage_and_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer sk-test-SECRET"))
        .respond_with(
            json_resp(json!({
                "id": "resp_1", "status": "completed",
                "output": [{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello"}]}],
                "usage": {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 40},
                          "output_tokens": 20, "output_tokens_details": {"reasoning_tokens": 5}}
            }))
            .insert_header("x-request-id", "req_abc"),
        )
        .mount(&server)
        .await;
    let mut req = simple_req("gpt-4.1");
    req.system = Some("be brief".into());
    req.max_output_tokens = Some(64);
    req.cache_hint = Some(CacheHint {
        stable_prefix_messages: 0,
        key: Some("ws-1".into()),
    });
    let resp = adapter(&server).complete(&req).await.unwrap();
    assert_eq!(resp.text(), "Hello");
    assert_eq!(resp.stop_reason, StopReason::End);
    assert_eq!(resp.provider_request_id.as_deref(), Some("req_abc"));
    assert_eq!(
        resp.usage,
        Usage {
            input_uncached: 60,
            cache_read: 40,
            cache_write: 0,
            output: 20,
            reasoning: 5
        }
    );
    let b = last_body(&server).await;
    assert_eq!(b["store"], false);
    assert_eq!(b["instructions"], "be brief");
    assert_eq!(b["max_output_tokens"], 64);
    assert_eq!(b["prompt_cache_key"], "ws-1");
    assert_eq!(
        b["input"][0]["content"][0],
        json!({"type":"input_text","text":"hi"})
    );
}

#[tokio::test]
async fn tool_call_roundtrip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "status": "completed",
            "output": [{"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"city\":\"Hanoi\"}"}],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "status": "completed",
            "output": [{"type":"message","content":[{"type":"output_text","text":"31C and sunny"}]}],
            "usage": {"input_tokens": 30, "output_tokens": 6}
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = Request::new("gpt-4.1", vec![Message::user("weather in Hanoi?")]);
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    let Part::ToolCall {
        id,
        name,
        args_json,
    } = &r1.parts[0]
    else {
        panic!("no tool call")
    };
    assert_eq!((id.as_str(), name.as_str()), ("call_1", "get_weather"));
    assert_eq!(args_json, "{\"city\":\"Hanoi\"}");

    let r2 = p
        .complete(&tool_followup("gpt-4.1", r1.parts.clone(), id))
        .await
        .unwrap();
    assert_eq!(r2.text(), "31C and sunny");
    let b = last_body(&server).await;
    assert_eq!(
        b["input"][1],
        json!({"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"city\":\"Hanoi\"}"})
    );
    assert_eq!(
        b["input"][2],
        json!({"type":"function_call_output","call_id":"call_1","output":"31C sunny"})
    );
}

#[tokio::test]
async fn json_schema_and_tool_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({"status":"completed","output":[]})))
        .mount(&server)
        .await;
    let mut req = simple_req("gpt-4.1");
    req.response_format = Some(schema_fmt());
    req.tools = vec![weather_tool()];
    req.tool_choice = ToolChoice::Named("get_weather".into());
    adapter(&server).complete(&req).await.unwrap();
    let b = last_body(&server).await;
    assert_eq!(b["text"]["format"]["type"], "json_schema");
    assert_eq!(b["text"]["format"]["name"], "decision");
    assert_eq!(b["text"]["format"]["strict"], true);
    assert_eq!(b["text"]["format"]["schema"]["required"], json!(["speak"]));
    assert_eq!(b["tools"][0]["type"], "function");
    assert_eq!(b["tools"][0]["name"], "get_weather");
    assert_eq!(b["tools"][0]["parameters"]["required"], json!(["city"]));
    assert_eq!(
        b["tool_choice"],
        json!({"type":"function","name":"get_weather"})
    );
}

#[tokio::test]
async fn streaming_text_tool_and_usage() {
    let server = MockServer::start().await;
    let body = sse(&[
        ("response.created", json!({"type":"response.created"})),
        (
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,
            "item":{"type":"function_call","call_id":"call_9","name":"get_weather","arguments":""}}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"city\":"}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"\"Hue\"}"}),
        ),
        (
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","delta":"Hel"}),
        ),
        (
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","delta":"lo"}),
        ),
        (
            "response.completed",
            json!({"type":"response.completed","response":{"status":"completed",
            "output":[{"type":"function_call"}],
            "usage":{"input_tokens":50,"input_tokens_details":{"cached_tokens":10},"output_tokens":7}}}),
        ),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    let ev = collect(&adapter(&server), &simple_req("gpt-4.1")).await;
    assert_eq!(
        ev,
        vec![
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_9".into()),
                name: Some("get_weather".into()),
                args_delta: String::new()
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "{\"city\":".into()
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "\"Hue\"}".into()
            },
            StreamEvent::TextDelta("Hel".into()),
            StreamEvent::TextDelta("lo".into()),
            StreamEvent::Usage(Usage {
                input_uncached: 40,
                cache_read: 10,
                cache_write: 0,
                output: 7,
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
async fn opaque_reasoning_item_is_echoed_unchanged() {
    let server = MockServer::start().await;
    let reasoning =
        json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ENC=="});
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "status":"completed",
            "output":[reasoning.clone(),
                {"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{}"}],
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("gpt-6-luna");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(
        r1.parts[0],
        Part::Opaque {
            provider: ProviderId::OpenAi,
            blob: reasoning.clone()
        }
    );
    p.complete(&tool_followup("gpt-6-luna", r1.parts.clone(), "call_1"))
        .await
        .unwrap();
    let b = last_body(&server).await;
    assert_eq!(b["input"][1], reasoning);
    assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));
}

#[tokio::test]
async fn embeddings_sorted_by_index() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(json_resp(json!({
            "data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}],
            "usage":{"prompt_tokens":4,"total_tokens":4}
        })))
        .mount(&server)
        .await;
    let e = adapter(&server)
        .embed("text-embedding-3-small", &["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(e.vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    assert_eq!(e.dims, 2);
    assert_eq!(e.usage.input_uncached, 4);
    assert_eq!(last_body(&server).await["model"], "text-embedding-3-small");
}

#[tokio::test]
async fn mid_stream_error_event_is_classified() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","delta":"x"}),
        ),
        (
            "error",
            json!({"type":"error","error":{"type":"server_is_overloaded","message":"busy"}}),
        ),
    ]);
    Mock::given(method("POST"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    use futures::StreamExt;
    let items: Vec<_> = adapter(&server)
        .stream(&simple_req("gpt-4.1"))
        .await
        .unwrap()
        .collect()
        .await;
    assert!(matches!(items[0], Ok(StreamEvent::TextDelta(_))));
    assert!(matches!(items[1], Err(LlmError::Overloaded(_))));
}
