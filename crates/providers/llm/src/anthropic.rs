//! Anthropic adapter: Messages API with prompt caching and preserved thinking blocks.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use crate::config::ProviderConfig;
use crate::error::LlmError;
use crate::http::{
    AuthStyle, HttpCore, SseParser, classify_stream_error, join_text, parse_json, sse_stream,
    tool_args, u32_at,
};
use crate::model::*;
use crate::provider::{Capabilities, LlmProvider, PromptCache};

const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    http: HttpCore,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("http", &self.http)
            .finish()
    }
}

/// Claude 4.7+/5.x style models. UNVERIFIED (doc marks it [V] for 5.5 only): forced tool_choice
/// is rejected and `temperature` below 1 is a 400 on these.
fn modern(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("-5-")
        || m.ends_with("-5")
        || m.contains("4-7")
        || m.contains("4-8")
        || m.contains("4-9")
}

/// Anthropic structured outputs require `additionalProperties: false` on every object.
fn close_objects(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if o.contains_key("properties") && !o.contains_key("additionalProperties") {
                o.insert("additionalProperties".into(), Value::Bool(false));
            }
            for x in o.values_mut() {
                close_objects(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(close_objects),
        _ => {}
    }
}

fn set_cache_control(block: &mut Value) {
    if let Some(o) = block.as_object_mut() {
        o.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
}

impl AnthropicProvider {
    pub fn new(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        Ok(Self {
            http: HttpCore::new(
                cfg,
                "https://api.anthropic.com/v1",
                AuthStyle::Header("x-api-key"),
                &[("anthropic-version", API_VERSION)],
            )?,
        })
    }

    fn build_body(&self, req: &Request, stream: bool) -> Result<Value, LlmError> {
        let caps = self.capabilities(&req.model);
        let prefix = req
            .cache_hint
            .as_ref()
            .map_or(0, |h| h.stable_prefix_messages);
        let mut system = req.system.clone().unwrap_or_default();
        let mut messages: Vec<Value> = Vec::new();
        let mut cache_idx: Option<usize> = None;

        for (i, m) in req.messages.iter().enumerate() {
            if m.role == Role::System {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&join_text(&m.parts));
                continue;
            }
            let role = if m.role == Role::Assistant {
                "assistant"
            } else {
                "user"
            };
            let mut results = Vec::new();
            let mut blocks = Vec::new();
            for p in &m.parts {
                match p {
                    Part::Text(t) if !t.is_empty() => blocks.push(json!({"type": "text", "text": t})),
                    Part::Text(_) => {}
                    Part::ToolCall { id, name, args_json } => blocks.push(json!({
                        "type": "tool_use", "id": id, "name": name, "input": tool_args(args_json, id)?,
                    })),
                    Part::ToolResult { id, content, is_error } => {
                        let mut b = json!({"type": "tool_result", "tool_use_id": id, "content": content});
                        if *is_error {
                            b["is_error"] = json!(true);
                        }
                        results.push(b);
                    }
                    Part::Opaque { provider, blob } => {
                        if *provider == ProviderId::Anthropic {
                            // Thinking blocks must be returned exactly as received.
                            blocks.push(blob.clone());
                        }
                    }
                }
            }
            // tool_result blocks must lead the user turn.
            results.extend(blocks);
            if !results.is_empty() {
                let same_role = messages
                    .last()
                    .is_some_and(|l| l.get("role").and_then(Value::as_str) == Some(role));
                if same_role {
                    if let Some(arr) = messages
                        .last_mut()
                        .and_then(|l| l.get_mut("content"))
                        .and_then(Value::as_array_mut)
                    {
                        arr.extend(results);
                    }
                } else {
                    messages.push(json!({"role": role, "content": results}));
                }
            }
            if prefix > 0 && i + 1 == prefix && !messages.is_empty() {
                cache_idx = Some(messages.len() - 1);
            }
        }
        let last_block = cache_idx.and_then(|ci| {
            messages[ci]
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.last_mut())
        });
        if let Some(last) = last_block {
            let ty = last.get("type").and_then(Value::as_str).unwrap_or("");
            if ty != "thinking" && ty != "redacted_thinking" {
                set_cache_control(last);
            }
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        body.insert(
            "max_tokens".into(),
            json!(req.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
        );
        body.insert("messages".into(), Value::Array(messages));
        if stream {
            body.insert("stream".into(), json!(true));
        }
        let cache = req.cache_hint.is_some();
        if !system.is_empty() {
            if cache {
                body.insert(
                    "system".into(),
                    json!([{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}]),
                );
            } else {
                body.insert("system".into(), json!(system));
            }
        }
        if !req.tools.is_empty() {
            let mut tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.json_schema}))
                .collect();
            let mark = if cache && system.is_empty() {
                tools.last_mut()
            } else {
                None
            };
            if let Some(last) = mark {
                set_cache_control(last);
            }
            body.insert("tools".into(), Value::Array(tools));
            let choice = match &req.tool_choice {
                ToolChoice::Auto => json!({"type": "auto"}),
                ToolChoice::None => json!({"type": "none"}),
                ToolChoice::Required | ToolChoice::Named(_) if !caps.forced_tool_choice => {
                    tracing::warn!(model = %req.model, "forced tool_choice unsupported; using auto");
                    json!({"type": "auto"})
                }
                ToolChoice::Required => json!({"type": "any"}),
                ToolChoice::Named(n) => json!({"type": "tool", "name": n}),
            };
            body.insert("tool_choice".into(), choice);
        }
        if let Some(f) = &req.response_format {
            let mut schema = f.schema.clone();
            close_objects(&mut schema);
            body.insert(
                "output_config".into(),
                json!({"format": {"type": "json_schema", "schema": schema}}),
            );
        }
        if let (Some(t), true) = (req.temperature, caps.accepts_temperature) {
            body.insert("temperature".into(), json!(t));
        }
        if let Some(u) = req.metadata.get("user_id") {
            body.insert("metadata".into(), json!({"user_id": u}));
        }
        Ok(Value::Object(body))
    }
}

fn stop_of(s: Option<&str>) -> StopReason {
    match s {
        Some("end_turn") | Some("stop_sequence") | None => StopReason::End,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Filtered,
        Some(_) => StopReason::Other,
    }
}

/// Anthropic's `input_tokens` already excludes cache reads and writes.
fn usage_of(u: &Value) -> Usage {
    Usage {
        input_uncached: u32_at(u, "/input_tokens"),
        cache_read: u32_at(u, "/cache_read_input_tokens"),
        cache_write: u32_at(u, "/cache_creation_input_tokens"),
        output: u32_at(u, "/output_tokens"),
        reasoning: 0, // not reported separately
    }
}

fn parse_response(v: &Value, rid: Option<String>) -> Result<Response, LlmError> {
    let mut parts = Vec::new();
    for b in v
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    parts.push(Part::Text(t.to_owned()));
                }
            }
            Some("tool_use") => parts.push(Part::ToolCall {
                id: b
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: b
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                args_json: b.get("input").unwrap_or(&json!({})).to_string(),
            }),
            // thinking, redacted_thinking and any block type we do not know: keep verbatim.
            _ => parts.push(Part::Opaque {
                provider: ProviderId::Anthropic,
                blob: b.clone(),
            }),
        }
    }
    Ok(Response {
        parts,
        stop_reason: stop_of(v.get("stop_reason").and_then(Value::as_str)),
        usage: v.get("usage").map(usage_of).unwrap_or_default(),
        provider_request_id: rid.or_else(|| v.get("id").and_then(Value::as_str).map(str::to_owned)),
    })
}

#[derive(Default)]
struct AnthropicStream {
    usage: Usage,
    stop: Option<String>,
    /// content block index -> (thinking text, signature)
    thinking: BTreeMap<u32, (String, String)>,
    done: bool,
}

impl SseParser for AnthropicStream {
    fn event(&mut self, _name: &str, data: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let v = parse_json(data)?;
        let mut out = Vec::new();
        let idx = u32_at(&v, "/index");
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.usage = usage_of(u);
                }
            }
            "content_block_start" => {
                let b = v.get("content_block").unwrap_or(&Value::Null);
                match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => out.push(StreamEvent::ToolCallDelta {
                        index: idx,
                        id: b.get("id").and_then(Value::as_str).map(str::to_owned),
                        name: b.get("name").and_then(Value::as_str).map(str::to_owned),
                        args_delta: String::new(),
                    }),
                    Some("thinking") => {
                        self.thinking.insert(idx, (String::new(), String::new()));
                    }
                    Some("redacted_thinking") => out.push(StreamEvent::Opaque {
                        provider: ProviderId::Anthropic,
                        blob: b.clone(),
                    }),
                    _ => {}
                }
            }
            "content_block_delta" => {
                let d = v.get("delta").unwrap_or(&Value::Null);
                match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = d.get("text").and_then(Value::as_str) {
                            out.push(StreamEvent::TextDelta(t.to_owned()));
                        }
                    }
                    Some("input_json_delta") => out.push(StreamEvent::ToolCallDelta {
                        index: idx,
                        id: None,
                        name: None,
                        args_delta: d
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    }),
                    Some("thinking_delta") => {
                        if let (Some(e), Some(t)) = (
                            self.thinking.get_mut(&idx),
                            d.get("thinking").and_then(Value::as_str),
                        ) {
                            e.0.push_str(t);
                        }
                    }
                    Some("signature_delta") => {
                        if let (Some(e), Some(t)) = (
                            self.thinking.get_mut(&idx),
                            d.get("signature").and_then(Value::as_str),
                        ) {
                            e.1.push_str(t);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some((text, sig)) = self.thinking.remove(&idx) {
                    out.push(StreamEvent::Opaque {
                        provider: ProviderId::Anthropic,
                        blob: json!({"type": "thinking", "thinking": text, "signature": sig}),
                    });
                }
            }
            "message_delta" => {
                if let Some(s) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(s.to_owned());
                }
                // message_delta usage is cumulative; fields may be absent, so only overwrite present ones.
                if let Some(u) = v.get("usage") {
                    for (key, slot) in [
                        ("output_tokens", &mut self.usage.output),
                        ("input_tokens", &mut self.usage.input_uncached),
                        ("cache_read_input_tokens", &mut self.usage.cache_read),
                        ("cache_creation_input_tokens", &mut self.usage.cache_write),
                    ] {
                        if u.get(key).is_some_and(|x| x.is_u64()) {
                            *slot = u32_at(u, &format!("/{key}"));
                        }
                    }
                }
            }
            "message_stop" => {
                self.done = true;
                out.push(StreamEvent::Usage(self.usage));
                out.push(StreamEvent::Done {
                    stop_reason: stop_of(self.stop.as_deref()),
                });
            }
            // Errors can arrive after HTTP 200.
            "error" => return Err(classify_stream_error(&v)),
            _ => {} // ping and unknown events
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        if self.done {
            Ok(Vec::new())
        } else {
            Err(LlmError::Transport(
                "stream ended before message_stop".into(),
            ))
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        let modern = modern(model);
        Capabilities {
            tools: true,
            parallel_tools: true,
            forced_tool_choice: !modern,
            json_schema: true,
            reasoning: true,
            accepts_temperature: !modern,
            prompt_cache: PromptCache::Explicit,
            streaming_tool_args: true,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let body = self.build_body(req, false)?;
        let (v, rid) = self
            .http
            .post_json(self.http.url(&["messages"])?, &body)
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
            .post_sse(self.http.url(&["messages"])?, &body)
            .await?;
        Ok(sse_stream(resp, AnthropicStream::default()))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        let mut url = self.http.url(&["models"])?;
        url.query_pairs_mut().append_pair("limit", "1");
        self.http.get_ok(url).await
    }
}
