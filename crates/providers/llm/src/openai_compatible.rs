//! OpenAI-compatible adapter (Chat Completions) for Azure OpenAI, gateways, vLLM, OpenRouter.
//! `base_url` and header overrides come from `ProviderConfig`; capabilities are conservative.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use crate::config::ProviderConfig;
use crate::error::LlmError;
use crate::http::{
    AuthStyle, HttpCore, SseParser, classify_stream_error, join_text, openai_usage, parse_json,
    sse_stream,
};
use crate::model::*;
use crate::provider::{Capabilities, LlmProvider};

pub struct OpenAiCompatibleProvider {
    http: HttpCore,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("http", &self.http)
            .finish()
    }
}

impl OpenAiCompatibleProvider {
    pub fn new(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        Ok(Self {
            http: HttpCore::new(cfg, "https://api.openai.com/v1", AuthStyle::Bearer, &[])?,
        })
    }

    fn build_body(&self, req: &Request, stream: bool) -> Result<Value, LlmError> {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(s) = &req.system {
            messages.push(json!({"role": "system", "content": s}));
        }
        for m in &req.messages {
            // Tool results become their own `tool` messages, whichever role carried them.
            for p in &m.parts {
                if let Part::ToolResult { id, content, .. } = p {
                    messages.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                }
            }
            match m.role {
                Role::System => {
                    messages.push(json!({"role": "system", "content": join_text(&m.parts)}))
                }
                Role::User | Role::Tool => {
                    let text = join_text(&m.parts);
                    if !text.is_empty() {
                        messages.push(json!({"role": "user", "content": text}));
                    }
                }
                Role::Assistant => {
                    let text = join_text(&m.parts);
                    let mut msg = Map::new();
                    msg.insert("role".into(), json!("assistant"));
                    msg.insert(
                        "content".into(),
                        if text.is_empty() {
                            Value::Null
                        } else {
                            json!(text)
                        },
                    );
                    let calls: Vec<Value> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::ToolCall {
                                id,
                                name,
                                args_json,
                            } => Some(json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": args_json},
                            })),
                            _ => None,
                        })
                        .collect();
                    if !calls.is_empty() {
                        msg.insert("tool_calls".into(), Value::Array(calls));
                    }
                    for p in &m.parts {
                        if let Part::Opaque {
                            provider: ProviderId::OpenAiCompatible,
                            blob: Value::Object(extra),
                        } = p
                        {
                            // UNVERIFIED: gateways such as DeepSeek want `reasoning_content` echoed
                            // on the assistant message; blob keys are merged back unchanged.
                            for (k, v) in extra {
                                msg.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    messages.push(Value::Object(msg));
                }
            }
        }
        let mut body = json!({"model": req.model, "messages": messages, "stream": stream});
        let o = body
            .as_object_mut()
            .ok_or_else(|| LlmError::Decode("body".into()))?;
        if stream {
            o.insert("stream_options".into(), json!({"include_usage": true}));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({"type": "function", "function": {
                        "name": t.name, "description": t.description, "parameters": t.json_schema,
                    }})
                })
                .collect();
            o.insert("tools".into(), Value::Array(tools));
            o.insert(
                "tool_choice".into(),
                match &req.tool_choice {
                    ToolChoice::Auto => json!("auto"),
                    ToolChoice::None => json!("none"),
                    ToolChoice::Required => json!("required"),
                    ToolChoice::Named(n) => json!({"type": "function", "function": {"name": n}}),
                },
            );
        }
        if let Some(f) = &req.response_format {
            o.insert(
                "response_format".into(),
                json!({"type": "json_schema", "json_schema": {
                    "name": f.name, "schema": f.schema, "strict": f.strict,
                }}),
            );
        }
        if let Some(n) = req.max_output_tokens {
            // UNVERIFIED: `max_tokens` is the most widely accepted name across gateways.
            o.insert("max_tokens".into(), json!(n));
        }
        if let Some(t) = req.temperature {
            o.insert("temperature".into(), json!(t));
        }
        Ok(body)
    }
}

fn stop_of(reason: Option<&str>, saw_tool: bool) -> StopReason {
    match reason {
        Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Filtered,
        Some("stop") | None => {
            if saw_tool {
                StopReason::ToolUse
            } else {
                StopReason::End
            }
        }
        Some(_) => StopReason::Other,
    }
}

fn usage_of(v: &Value) -> Usage {
    v.get("usage")
        .map(|u| {
            openai_usage(
                u,
                "prompt_tokens",
                "completion_tokens",
                "prompt_tokens_details",
                "completion_tokens_details",
            )
        })
        .unwrap_or_default()
}

fn parse_response(v: &Value, rid: Option<String>) -> Result<Response, LlmError> {
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| LlmError::Decode("response has no choices".into()))?;
    let msg = choice.get("message").unwrap_or(&Value::Null);
    let mut parts = Vec::new();
    let nonempty = |k: &str| {
        msg.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(r) = nonempty("reasoning_content") {
        parts.push(Part::Opaque {
            provider: ProviderId::OpenAiCompatible,
            blob: json!({"reasoning_content": r}),
        });
    }
    if let Some(t) = nonempty("content") {
        parts.push(Part::Text(t));
    }
    let mut saw_tool = false;
    for c in msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        saw_tool = true;
        parts.push(Part::ToolCall {
            id: c
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            name: c
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            args_json: c
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_owned(),
        });
    }
    Ok(Response {
        parts,
        stop_reason: stop_of(
            choice.get("finish_reason").and_then(Value::as_str),
            saw_tool,
        ),
        usage: usage_of(v),
        provider_request_id: rid.or_else(|| v.get("id").and_then(Value::as_str).map(str::to_owned)),
    })
}

#[derive(Default)]
struct ChatStream {
    stop: Option<StopReason>,
    saw_tool: bool,
    usage: Option<Usage>,
    reasoning: String,
    done: bool,
}

impl ChatStream {
    fn wrap_up(&mut self) -> Vec<StreamEvent> {
        if self.done {
            return Vec::new();
        }
        self.done = true;
        let mut out = Vec::new();
        if !self.reasoning.is_empty() {
            out.push(StreamEvent::Opaque {
                provider: ProviderId::OpenAiCompatible,
                blob: json!({"reasoning_content": std::mem::take(&mut self.reasoning)}),
            });
        }
        if let Some(u) = self.usage.take() {
            out.push(StreamEvent::Usage(u));
        }
        let stop = self.stop.unwrap_or(if self.saw_tool {
            StopReason::ToolUse
        } else {
            StopReason::End
        });
        out.push(StreamEvent::Done { stop_reason: stop });
        out
    }
}

impl SseParser for ChatStream {
    fn event(&mut self, _name: &str, data: &str) -> Result<Vec<StreamEvent>, LlmError> {
        if data.trim() == "[DONE]" {
            return Ok(self.wrap_up());
        }
        let v = parse_json(data)?;
        if v.get("error").is_some() {
            return Err(classify_stream_error(&v));
        }
        let mut out = Vec::new();
        if v.get("usage").is_some_and(|u| !u.is_null()) {
            self.usage = Some(usage_of(&v));
        }
        if let Some(choice) = v.pointer("/choices/0") {
            let delta = choice.get("delta").unwrap_or(&Value::Null);
            if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.reasoning.push_str(r);
            }
            let text = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty());
            if let Some(t) = text {
                out.push(StreamEvent::TextDelta(t.to_owned()));
            }
            for c in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.saw_tool = true;
                out.push(StreamEvent::ToolCallDelta {
                    index: c.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                    id: c.get("id").and_then(Value::as_str).map(str::to_owned),
                    name: c
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    args_delta: c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                });
            }
            if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                self.stop = Some(stop_of(Some(fr), self.saw_tool));
            }
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        Ok(self.wrap_up())
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAiCompatible
    }

    /// Conservative: unknown gateways are assumed text-only until a probe says otherwise.
    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities::conservative()
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let body = self.build_body(req, false)?;
        let url = self.http.url(&["chat", "completions"])?;
        let (v, rid) = self.http.post_json(url, &body).await?;
        parse_response(&v, rid)
    }

    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        let body = self.build_body(req, true)?;
        let url = self.http.url(&["chat", "completions"])?;
        let resp = self.http.post_sse(url, &body).await?;
        Ok(sse_stream(resp, ChatStream::default()))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        self.http.get_ok(self.http.url(&["models"])?).await
    }
}
