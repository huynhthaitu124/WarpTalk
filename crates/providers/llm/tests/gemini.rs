#![allow(clippy::unwrap_used)]
mod common;

use common::*;
use serde_json::json;
use warptalk_llm::gemini::GeminiProvider;
use warptalk_llm::*;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer};

const KIND: ProviderId = ProviderId::Gemini;
const GEN: &str = "/v1beta/models/gemini-3.5-flash:generateContent";

fn adapter(server: &MockServer) -> GeminiProvider {
    GeminiProvider::new(&cfg(KIND, &server.uri())).unwrap()
}

#[tokio::test]
async fn text_completion_and_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(GEN))
        .and(header("x-goog-api-key", "sk-test-SECRET"))
        .respond_with(json_resp(json!({
            "candidates":[{"content":{"role":"model","parts":[{"text":"Xin chao"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":30,
                             "candidatesTokenCount":10,"thoughtsTokenCount":4,"totalTokenCount":114},
            "responseId":"r1", "modelVersion":"x"
        })))
        .mount(&server)
        .await;
    let mut req = simple_req("gemini-3.5-flash");
    req.system = Some("sys".into());
    req.max_output_tokens = Some(50);
    let resp = adapter(&server).complete(&req).await.unwrap();
    assert_eq!(resp.text(), "Xin chao");
    assert_eq!(resp.stop_reason, StopReason::End);
    assert_eq!(resp.provider_request_id.as_deref(), Some("r1"));
    assert_eq!(
        resp.usage,
        Usage {
            input_uncached: 70,
            cache_read: 30,
            cache_write: 0,
            output: 14,
            reasoning: 4
        }
    );
    let b = last_body(&server).await;
    assert_eq!(b["systemInstruction"], json!({"parts":[{"text":"sys"}]}));
    assert_eq!(
        b["contents"][0],
        json!({"role":"user","parts":[{"text":"hi"}]})
    );
    assert_eq!(b["generationConfig"]["maxOutputTokens"], 50);
    // Key must not leak into the URL.
    let reqs = server.received_requests().await.unwrap();
    assert!(!reqs[0].url.as_str().contains("SECRET"));
}

#[tokio::test]
async fn tool_call_roundtrip_with_name_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"get_weather","args":{"city":"Hanoi"}}}]},"finishReason":"STOP"}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "candidates":[{"content":{"parts":[{"text":"31C"}]},"finishReason":"STOP"}]
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("gemini-3.5-flash");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    let b = last_body(&server).await;
    assert_eq!(
        b["tools"][0]["functionDeclarations"][0]["name"],
        "get_weather"
    );
    assert_eq!(b["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    let Part::ToolCall { id, .. } = &r1.parts[0] else {
        panic!("no call")
    };
    assert!(id.starts_with("gemini-synth-"));

    let r2 = p
        .complete(&tool_followup("gemini-3.5-flash", r1.parts.clone(), id))
        .await
        .unwrap();
    assert_eq!(r2.text(), "31C");
    let b = last_body(&server).await;
    // Synthetic ids are not sent back; the function name is recovered from the earlier call.
    assert_eq!(
        b["contents"][1],
        json!({"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"Hanoi"}}}]})
    );
    assert_eq!(
        b["contents"][2],
        json!({"role":"user","parts":[{"functionResponse":{"name":"get_weather","response":{"output":"31C sunny"}}}]})
    );
}

#[tokio::test]
async fn json_schema_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(
            json!({"candidates":[{"content":{"parts":[{"text":"{}"}]}}]}),
        ))
        .mount(&server)
        .await;
    let mut req = simple_req("gemini-3.5-flash");
    req.response_format = Some(schema_fmt());
    req.tools = vec![weather_tool()];
    req.tool_choice = ToolChoice::Named("get_weather".into());
    adapter(&server).complete(&req).await.unwrap();
    let b = last_body(&server).await;
    assert_eq!(
        b["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        b["generationConfig"]["responseJsonSchema"]["required"],
        json!(["speak"])
    );
    assert_eq!(
        b["toolConfig"]["functionCallingConfig"],
        json!({"mode":"ANY","allowedFunctionNames":["get_weather"]})
    );
}

#[tokio::test]
async fn streaming_sse_with_usage_and_function_call() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "",
            json!({"candidates":[{"content":{"parts":[{"text":"Xin "}]}}]}),
        ),
        (
            "",
            json!({"candidates":[{"content":{"parts":[{"text":"chao"}]}}]}),
        ),
        (
            "",
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"fc1","name":"get_weather","args":{"city":"Hue"}}}]},
            "finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":20,"cachedContentTokenCount":5,"candidatesTokenCount":6}}),
        ),
    ]);
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3.5-flash:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_resp(body))
        .mount(&server)
        .await;
    let ev = collect(&adapter(&server), &simple_req("gemini-3.5-flash")).await;
    assert_eq!(
        ev,
        vec![
            StreamEvent::TextDelta("Xin ".into()),
            StreamEvent::TextDelta("chao".into()),
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("fc1".into()),
                name: Some("get_weather".into()),
                args_delta: "{\"city\":\"Hue\"}".into()
            },
            StreamEvent::Usage(Usage {
                input_uncached: 15,
                cache_read: 5,
                cache_write: 0,
                output: 6,
                reasoning: 0
            }),
            StreamEvent::Done {
                stop_reason: StopReason::ToolUse
            },
        ]
    );
}

#[tokio::test]
async fn thought_signature_is_echoed_on_the_same_part() {
    let server = MockServer::start().await;
    let summary = json!({"text":"thinking...","thought":true});
    Mock::given(method("POST"))
        .respond_with(json_resp(json!({
            "candidates":[{"content":{"parts":[
                summary.clone(),
                {"functionCall":{"id":"fc1","name":"get_weather","args":{}},"thoughtSignature":"SIG123"}]},
                "finishReason":"STOP"}]
        })))
        .mount(&server)
        .await;
    let p = adapter(&server);
    let mut req = simple_req("gemini-3.5-flash");
    req.tools = vec![weather_tool()];
    let r1 = p.complete(&req).await.unwrap();
    assert!(
        r1.parts.iter().any(
            |x| matches!(x, Part::Opaque { blob, .. } if blob["thoughtSignature"] == "SIG123")
        )
    );
    p.complete(&tool_followup("gemini-3.5-flash", r1.parts.clone(), "fc1"))
        .await
        .unwrap();
    let parts = &last_body(&server).await["contents"][1]["parts"];
    assert_eq!(parts[0], summary);
    assert_eq!(parts[1]["functionCall"]["id"], "fc1");
    assert_eq!(parts[1]["thoughtSignature"], "SIG123");
    assert_eq!(parts.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn blocked_prompt_is_content_filtered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_resp(
            json!({"promptFeedback":{"blockReason":"SAFETY"}}),
        ))
        .mount(&server)
        .await;
    let e = adapter(&server)
        .complete(&simple_req("gemini-3.5-flash"))
        .await
        .unwrap_err();
    assert!(matches!(e, LlmError::ContentFiltered(_)));
}

#[tokio::test]
async fn embeddings_batch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-embedding-001:batchEmbedContents",
        ))
        .respond_with(json_resp(
            json!({"embeddings":[{"values":[0.5,0.25,0.0]},{"values":[1.0,0.0,0.0]}]}),
        ))
        .mount(&server)
        .await;
    let e = adapter(&server)
        .embed("gemini-embedding-001", &["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(e.dims, 3);
    assert_eq!(e.vectors.len(), 2);
    let b = last_body(&server).await;
    assert_eq!(b["requests"][1]["model"], "models/gemini-embedding-001");
    assert_eq!(b["requests"][1]["content"]["parts"][0]["text"], "b");
}
