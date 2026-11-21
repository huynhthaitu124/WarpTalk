//! OpenAI adapter: Responses API (`store=false`) plus embeddings.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::LlmError;
use crate::http::{
    AuthStyle, HttpCore, SseParser, classify_stream_error, openai_usage, parse_json, sse_stream,
    u32_at,
};
use crate::model::*;
use crate::provider::{Capabilities, EmbeddingProvider, LlmProvider, PromptCache};

pub struct OpenAiProvider {
    http: HttpCore,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("http", &self.http)
            .finish()
    }
}

fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("gpt-5")
        || m.starts_with("gpt-6")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
}

impl OpenAiProvider {
    pub fn new(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        Ok(Self {
            http: HttpCore::new(cfg, "https://api.openai.com/v1", AuthStyle::Bearer, &[])?,
        })
    }

    fn build_body(&self, req: &Request, stream: bool) -> Result<Value, LlmError> {
        let mut input: Vec<Value> = Vec::new();
        for m in &req.messages {
            for p in &m.parts {
                match p {
                    Part::Text(t) => input.push(match m.role {
                        Role::Assistant => json!({
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": t}],
                        }),
                        Role::System => json!({
                            "role": "system",
                            "content": [{"type": "input_text", "text": t}],
                        }),
                        _ => json!({
                            "role": "user",
                            "content": [{"type": "input_text", "text": t}],
                        }),
                    }),
                    Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } => input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": args_json,
                    })),
                    Part::ToolResult { id, content, .. } => input.push(json!({
                        "type": "function_call_output",
                        "call_id": id,
                        "output": content,
                    })),
                    Part::Opaque { provider, blob } => {
                        if *provider == ProviderId::OpenAi {
                            // UNVERIFIED: with store=false a reasoning item (incl. encrypted_content)
                            // is accepted verbatim as an input item.
                            input.push(blob.clone());
                        }
                    }
                }
            }
        }
        let mut body = json!({
            "model": req.model,
            "input": input,
            "store": false,
            "stream": stream,
        });
        let o = body
            .as_object_mut()
            .ok_or_else(|| LlmError::Decode("body".into()))?;
        if let Some(s) = &req.system {
            o.insert("instructions".into(), json!(s));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.json_schema,
                    })
                })
                .collect();
            o.insert("tools".into(), Value::Array(tools));
            // UNVERIFIED: Responses `tool_choice` values (auto/none/required/{type:function,name}).
            o.insert(
                "tool_choice".into(),
                match &req.tool_choice {
                    ToolChoice::Auto => json!("auto"),
                    ToolChoice::None => json!("none"),
                    ToolChoice::Required => json!("required"),
                    ToolChoice::Named(n) => json!({"type": "function", "name": n}),
                },
            );
        }
        if let Some(f) = &req.response_format {
            o.insert(
                "text".into(),
                json!({"format": {
                    "type": "json_schema",
                    "name": f.name,
                    "schema": f.schema,
                    "strict": f.strict,
                }}),
            );
        }
        if let Some(n) = req.max_output_tokens {
            o.insert("max_output_tokens".into(), json!(n));
        }
        if let (Some(t), true) = (
            req.temperature,
            self.capabilities(&req.model).accepts_temperature,
        ) {
            o.insert("temperature".into(), json!(t));
        }
        if let Some(k) = req.cache_hint.as_ref().and_then(|h| h.key.as_ref()) {
            o.insert("prompt_cache_key".into(), json!(k));
        }
        if is_reasoning_model(&req.model) {
            // UNVERIFIED: needed to round-trip reasoning items when store=false.
            o.insert("include".into(), json!(["reasoning.encrypted_content"]));
        }
        Ok(body)
    }
}

fn stop_reason(resp: &Value, saw_tool: bool) -> StopReason {
    if resp.get("status").and_then(Value::as_str) == Some("incomplete") {
        return match resp
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Filtered,
            _ => StopReason::Other,
        };
    }
    if saw_tool {
        StopReason::ToolUse
    } else {
        StopReason::End
    }
}

fn usage_of(resp: &Value) -> Usage {
    resp.get("usage")
        .map(|u| {
            openai_usage(
                u,
                "input_tokens",
                "output_tokens",
                "input_tokens_details",
                "output_tokens_details",
            )
        })
        .unwrap_or_default()
}

fn parse_response(v: &Value, rid: Option<String>) -> Result<Response, LlmError> {
    if v.get("status").and_then(Value::as_str) == Some("failed") {
        return Err(classify_stream_error(v));
    }
    let mut parts = Vec::new();
    let mut saw_tool = false;
    let mut refused = false;
    for item in v
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for c in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match c.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            if let Some(t) = c.get("text").and_then(Value::as_str) {
                                parts.push(Part::Text(t.to_owned()));
                            }
                        }
                        Some("refusal") => {
                            refused = true;
                            if let Some(t) = c.get("refusal").and_then(Value::as_str) {
                                parts.push(Part::Text(t.to_owned()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                saw_tool = true;
                parts.push(Part::ToolCall {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    args_json: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_owned(),
                });
            }
            Some("reasoning") => parts.push(Part::Opaque {
                provider: ProviderId::OpenAi,
                blob: item.clone(),
            }),
            _ => {}
        }
    }
    let stop = if refused {
        StopReason::Filtered
    } else {
        stop_reason(v, saw_tool)
    };
    Ok(Response {
        parts,
        stop_reason: stop,
        usage: usage_of(v),
        provider_request_id: rid.or_else(|| v.get("id").and_then(Value::as_str).map(str::to_owned)),
    })
}

#[derive(Default)]
struct OpenAiStream {
    saw_tool: bool,
    done: bool,
}

impl SseParser for OpenAiStream {
    fn event(&mut self, _name: &str, data: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let v = parse_json(data)?;
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    out.push(StreamEvent::TextDelta(d.to_owned()));
                }
            }
            "response.output_item.added" => {
                let item = v.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    self.saw_tool = true;
                    out.push(StreamEvent::ToolCallDelta {
                        index: u32_at(&v, "/output_index"),
                        id: item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        name: item.get("name").and_then(Value::as_str).map(str::to_owned),
                        args_delta: String::new(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    out.push(StreamEvent::ToolCallDelta {
                        index: u32_at(&v, "/output_index"),
                        id: None,
                        name: None,
                        args_delta: d.to_owned(),
                    });
                }
            }
            "response.output_item.done" => {
                let item = v.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                    out.push(StreamEvent::Opaque {
                        provider: ProviderId::OpenAi,
                        blob: item.clone(),
                    });
                }
            }
            "response.completed" | "response.incomplete" => {
                let resp = v.get("response").unwrap_or(&Value::Null);
                self.done = true;
                out.push(StreamEvent::Usage(usage_of(resp)));
                out.push(StreamEvent::Done {
                    stop_reason: stop_reason(resp, self.saw_tool),
                });
            }
            "response.failed" => {
                let resp = v.get("response").unwrap_or(&Value::Null);
                return Err(classify_stream_error(resp));
            }
            "error" => return Err(classify_stream_error(&v)),
            _ => {}
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        if self.done {
            Ok(Vec::new())
        } else {
            Err(LlmError::Transport(
                "stream ended before response.completed".into(),
            ))
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        Capabilities {
            tools: true,
            parallel_tools: true,
            forced_tool_choice: true,
            json_schema: true,
            reasoning: is_reasoning_model(model),
            // UNVERIFIED: reasoning models reject a custom temperature.
            accepts_temperature: !is_reasoning_model(model),
            prompt_cache: PromptCache::Implicit,
            streaming_tool_args: true,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let body = self.build_body(req, false)?;
        let (v, rid) = self
            .http
            .post_json(self.http.url(&["responses"])?, &body)
            .await?;
        parse_response(&v, rid)
    }

    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        let body = self.build_body(req, true)?;
        let resp = self
            .http
            .post_sse(self.http.url(&["responses"])?, &body)
            .await?;
        Ok(sse_stream(resp, OpenAiStream::default()))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        self.http.get_ok(self.http.url(&["models"])?).await
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiProvider {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        let body = json!({"model": model, "input": inputs, "encoding_format": "float"});
        let (v, _) = self
            .http
            .post_json(self.http.url(&["embeddings"])?, &body)
            .await?;
        let mut rows: Vec<(u64, Vec<f32>)> = Vec::new();
        for (i, d) in v
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let idx = d.get("index").and_then(Value::as_u64).unwrap_or(i as u64);
            let vec = d
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| LlmError::Decode("missing embedding".into()))?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            rows.push((idx, vec));
        }
        rows.sort_by_key(|(i, _)| *i);
        let vectors: Vec<Vec<f32>> = rows.into_iter().map(|(_, v)| v).collect();
        Ok(Embeddings {
            dims: vectors.first().map_or(0, Vec::len),
            vectors,
            usage: Usage {
                input_uncached: u32_at(&v, "/usage/prompt_tokens"),
                ..Usage::default()
            },
        })
    }
}
