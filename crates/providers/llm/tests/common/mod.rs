#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;

use futures::StreamExt;
use serde_json::{Value, json};
use warptalk_llm::*;
use wiremock::{MockServer, ResponseTemplate};

pub fn base_for(kind: ProviderId, uri: &str) -> String {
    match kind {
        ProviderId::Gemini => format!("{uri}/v1beta"),
        _ => format!("{uri}/v1"),
    }
}

pub fn cfg(kind: ProviderId, uri: &str) -> ProviderConfig {
    let mut c = ProviderConfig::new(kind, "sk-test-SECRET");
    c.base_url = Some(base_for(kind, uri));
    c.max_retries = 0;
    c
}

/// Provider built through the registry (Timeout wrapper, no retries).
pub fn provider(kind: ProviderId, server: &MockServer) -> Arc<dyn LlmProvider> {
    ProviderRegistry::build(cfg(kind, &server.uri())).unwrap()
}

pub fn simple_req(model: &str) -> Request {
    Request::new(model, vec![Message::user("hi")])
}

pub fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(n, d)| {
            if n.is_empty() {
                format!("data: {d}\n\n")
            } else {
                format!("event: {n}\ndata: {d}\n\n")
            }
        })
        .collect()
}

pub fn sse_resp(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

pub fn json_resp(v: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(v)
}

pub async fn body_n(server: &MockServer, n: usize) -> Value {
    let reqs = server.received_requests().await.unwrap();
    serde_json::from_slice(&reqs[n].body).unwrap()
}

pub async fn last_body(server: &MockServer) -> Value {
    let n = server.received_requests().await.unwrap().len();
    body_n(server, n - 1).await
}

pub async fn collect(p: &dyn LlmProvider, req: &Request) -> Vec<StreamEvent> {
    let s = p.stream(req).await.unwrap();
    s.map(|r| r.unwrap()).collect().await
}

pub fn weather_tool() -> ToolSpec {
    ToolSpec {
        name: "get_weather".into(),
        description: "Weather".into(),
        json_schema: json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}),
    }
}

pub fn schema_fmt() -> JsonSchemaFormat {
    JsonSchemaFormat {
        name: "decision".into(),
        schema: json!({"type":"object","properties":{"speak":{"type":"boolean"}},"required":["speak"]}),
        strict: true,
    }
}

/// Request after a tool call round: user, assistant(tool call), tool result.
pub fn tool_followup(model: &str, call_parts: Vec<Part>, id: &str) -> Request {
    let mut r = Request::new(
        model,
        vec![
            Message::user("weather in Hanoi?"),
            Message::assistant(call_parts),
            Message::tool_result(id, "31C sunny", false),
        ],
    );
    r.tools = vec![weather_tool()];
    r
}
