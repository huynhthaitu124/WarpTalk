//! Gemini adapter: generateContent / streamGenerateContent (`alt=sse`) plus embeddings.
//! The key goes in the `x-goog-api-key` header, never in the URL.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use crate::config::ProviderConfig;
use crate::error::LlmError;
use crate::http::{
    AuthStyle, HttpCore, SseParser, join_text, parse_json, sse_stream, tool_args, u32_at,
};
use crate::model::*;
use crate::provider::{Capabilities, EmbeddingProvider, LlmProvider, PromptCache};

/// Prefix for ids we invent when Gemini sends a function call without one; never echoed back.
const SYNTH_ID: &str = "gemini-synth-";

pub struct GeminiProvider {
    http: HttpCore,
}

impl std::fmt::Debug for GeminiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("http", &self.http)
            .finish()
    }
}

fn model_segment(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
}

/// `{"thoughtSignature": ...}` alone: a signature to re-attach to the preceding part.
fn is_signature_only(blob: &Value) -> bool {
    blob.as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("thoughtSignature"))
}

impl GeminiProvider {
    pub fn new(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        Ok(Self {
            http: HttpCore::new(
                cfg,
                "https://generativelanguage.googleapis.com/v1beta",
                AuthStyle::Header("x-goog-api-key"),
                &[],
            )?,
        })
    }

    fn build_body(&self, req: &Request) -> Result<Value, LlmError> {
        // functionResponse needs the function *name*, which ToolResult does not carry.
        let mut names: HashMap<&str, &str> = HashMap::new();
        for m in &req.messages {
            for p in &m.parts {
                if let Part::ToolCall { id, name, .. } = p {
                    names.insert(id, name);
                }
            }
        }
        let mut system = req.system.clone().unwrap_or_default();
        let mut contents: Vec<Value> = Vec::new();
        for m in &req.messages {
            if m.role == Role::System {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&join_text(&m.parts));
                continue;
            }
            let role = if m.role == Role::Assistant {
                "model"
            } else {
                "user"
            };
            let mut parts: Vec<Value> = Vec::new();
            for p in &m.parts {
                match p {
                    Part::Text(t) => parts.push(json!({"text": t})),
                    Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } => {
                        let mut fc = json!({"name": name, "args": tool_args(args_json, id)?});
                        if !id.starts_with(SYNTH_ID) {
                            fc["id"] = json!(id);
                        }
                        parts.push(json!({"functionCall": fc}));
                    }
                    Part::ToolResult {
                        id,
                        content,
                        is_error,
                    } => {
                        let name = names.get(id.as_str()).ok_or_else(|| {
                            LlmError::InvalidRequest(format!(
                                "tool result {id} has no matching tool call"
                            ))
                        })?;
                        let key = if *is_error { "error" } else { "output" };
                        let mut fr = json!({"name": name, "response": {key: content}});
                        if !id.starts_with(SYNTH_ID) {
                            fr["id"] = json!(id);
                        }
                        parts.push(json!({"functionResponse": fr}));
                    }
                    Part::Opaque { provider, blob } => {
                        if *provider != ProviderId::Gemini {
                            continue;
                        }
                        let attach = is_signature_only(blob)
                            && parts.last().is_some_and(|l| {
                                l.is_object() && l.get("thoughtSignature").is_none()
                            });
                        if attach {
                            if let (Some(last), Some(sig)) = (
                                parts.last_mut().and_then(Value::as_object_mut),
                                blob.get("thoughtSignature"),
                            ) {
                                last.insert("thoughtSignature".into(), sig.clone());
                            }
                        } else {
                            parts.push(blob.clone());
                        }
                    }
                }
            }
            if parts.is_empty() {
                continue;
            }
            let same = contents
                .last()
                .is_some_and(|l| l.get("role").and_then(Value::as_str) == Some(role));
            if same {
                if let Some(arr) = contents
                    .last_mut()
                    .and_then(|l| l.get_mut("parts"))
                    .and_then(Value::as_array_mut)
                {
                    arr.extend(parts);
                }
            } else {
                contents.push(json!({"role": role, "parts": parts}));
            }
        }

        let mut body = Map::new();
        body.insert("contents".into(), Value::Array(contents));
        if !system.is_empty() {
            body.insert(
                "systemInstruction".into(),
                json!({"parts": [{"text": system}]}),
            );
        }
        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    // UNVERIFIED: `parameters` takes an OpenAPI-subset schema (`parametersJsonSchema`
                    // may accept full JSON Schema).
                    json!({"name": t.name, "description": t.description, "parameters": t.json_schema})
                })
                .collect();
            body.insert("tools".into(), json!([{"functionDeclarations": decls}]));
            let cfg = match &req.tool_choice {
                ToolChoice::Auto => json!({"mode": "AUTO"}),
                ToolChoice::None => json!({"mode": "NONE"}),
                ToolChoice::Required => json!({"mode": "ANY"}),
                ToolChoice::Named(n) => json!({"mode": "ANY", "allowedFunctionNames": [n]}),
            };
            body.insert("toolConfig".into(), json!({"functionCallingConfig": cfg}));
        }
        let mut gcfg = Map::new();
        if let Some(n) = req.max_output_tokens {
            gcfg.insert("maxOutputTokens".into(), json!(n));
        }
        if let Some(t) = req.temperature {
            gcfg.insert("temperature".into(), json!(t));
        }
        if let Some(f) = &req.response_format {
            gcfg.insert("responseMimeType".into(), json!("application/json"));
            // UNVERIFIED: `responseJsonSchema` (JSON Schema) in generationConfig; older API used `responseSchema`.
            gcfg.insert("responseJsonSchema".into(), f.schema.clone());
        }
        if !gcfg.is_empty() {
            body.insert("generationConfig".into(), Value::Object(gcfg));
        }
        Ok(Value::Object(body))
    }

    fn gen_url(&self, model: &str, method: &str) -> Result<reqwest::Url, LlmError> {
        let seg = format!("{}:{method}", model_segment(model));
        self.http.url(&["models", &seg])
    }
}

/// Converts response parts. Thought signatures become a signature-only `Opaque` right after the
/// part they belong to; thought summaries and unknown parts are kept as raw `Opaque` parts.
fn parse_parts(parts: &[Value]) -> Vec<Part> {
    let mut out = Vec::new();
    for p in parts {
        let sig = p.get("thoughtSignature").cloned();
        if p.get("thought").and_then(Value::as_bool) == Some(true) {
            out.push(Part::Opaque {
                provider: ProviderId::Gemini,
                blob: p.clone(),
            });
            continue;
        }
        if let Some(fc) = p.get("functionCall") {
            let id = fc
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{SYNTH_ID}{}", uuid::Uuid::new_v4()));
            out.push(Part::ToolCall {
                id,
                name: fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                args_json: fc.get("args").unwrap_or(&json!({})).to_string(),
            });
        } else if let Some(t) = p.get("text").and_then(Value::as_str) {
            if !t.is_empty() || sig.is_some() {
                out.push(Part::Text(t.to_owned()));
            }
        } else if sig.is_none() {
            out.push(Part::Opaque {
                provider: ProviderId::Gemini,
                blob: p.clone(),
            });
            continue;
        }
        if let Some(sig) = sig {
            out.push(Part::Opaque {
                provider: ProviderId::Gemini,
                blob: json!({"thoughtSignature": sig}),
            });
        }
    }
    out
}

fn stop_of(reason: Option<&str>, saw_tool: bool) -> StopReason {
    match reason {
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some("SAFETY")
        | Some("RECITATION")
        | Some("BLOCKLIST")
        | Some("PROHIBITED_CONTENT")
        | Some("SPII")
        | Some("IMAGE_SAFETY") => StopReason::Filtered,
        Some("STOP") | None => {
            if saw_tool {
                StopReason::ToolUse
            } else {
                StopReason::End
            }
        }
        Some(_) => StopReason::Other,
    }
}

/// UNVERIFIED: `promptTokenCount` includes `cachedContentTokenCount`; thought tokens are billed as output.
fn usage_of(v: &Value) -> Usage {
    let Some(u) = v.get("usageMetadata") else {
        return Usage::default();
    };
    let prompt = u32_at(u, "/promptTokenCount");
    let cached = u32_at(u, "/cachedContentTokenCount");
    let thoughts = u32_at(u, "/thoughtsTokenCount");
    Usage {
        input_uncached: prompt.saturating_sub(cached),
        cache_read: cached,
        cache_write: 0,
        output: u32_at(u, "/candidatesTokenCount").saturating_add(thoughts),
        reasoning: thoughts,
    }
}

fn check_block(v: &Value) -> Result<(), LlmError> {
    if let Some(r) = v
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        return Err(LlmError::ContentFiltered(format!("prompt blocked: {r}")));
    }
    Ok(())
}

fn parse_response(v: &Value, rid: Option<String>) -> Result<Response, LlmError> {
    check_block(v)?;
    let cand = v.pointer("/candidates/0");
    let raw: &[Value] = cand
        .and_then(|c| c.pointer("/content/parts"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let parts = parse_parts(raw);
    let saw_tool = parts.iter().any(|p| matches!(p, Part::ToolCall { .. }));
    Ok(Response {
        parts,
        stop_reason: stop_of(
            cand.and_then(|c| c.get("finishReason"))
                .and_then(Value::as_str),
            saw_tool,
        ),
        usage: usage_of(v),
        provider_request_id: rid.or_else(|| {
            v.get("responseId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
    })
}

#[derive(Default)]
struct GeminiStream {
    next_call: u32,
    saw_tool: bool,
    finish: Option<String>,
    usage: Option<Usage>,
    done: bool,
}

impl SseParser for GeminiStream {
    fn event(&mut self, _name: &str, data: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let v = parse_json(data)?;
        if v.get("error").is_some() {
            return Err(crate::http::classify_stream_error(&v));
        }
        check_block(&v)?;
        let mut out = Vec::new();
        if v.get("usageMetadata").is_some() {
            self.usage = Some(usage_of(&v));
        }
        if let Some(c) = v.pointer("/candidates/0") {
            if let Some(f) = c.get("finishReason").and_then(Value::as_str) {
                self.finish = Some(f.to_owned());
            }
            let raw: &[Value] = c
                .pointer("/content/parts")
                .and_then(Value::as_array)
                .map_or(&[], Vec::as_slice);
            for p in parse_parts(raw) {
                match p {
                    Part::Text(t) if !t.is_empty() => out.push(StreamEvent::TextDelta(t)),
                    Part::Text(_) => {}
                    // Gemini sends a function call whole, in one chunk.
                    Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } => {
                        self.saw_tool = true;
                        out.push(StreamEvent::ToolCallDelta {
                            index: self.next_call,
                            id: Some(id),
                            name: Some(name),
                            args_delta: args_json,
                        });
                        self.next_call += 1;
                    }
                    Part::Opaque { provider, blob } => {
                        out.push(StreamEvent::Opaque { provider, blob })
                    }
                    Part::ToolResult { .. } => {}
                }
            }
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        if self.done {
            return Ok(Vec::new());
        }
        self.done = true;
        let mut out = Vec::new();
        if let Some(u) = self.usage.take() {
            out.push(StreamEvent::Usage(u));
        }
        out.push(StreamEvent::Done {
            stop_reason: stop_of(self.finish.as_deref(), self.saw_tool),
        });
        Ok(out)
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities {
            tools: true,
            parallel_tools: true,
            forced_tool_choice: true,
            json_schema: true,
            reasoning: true,
            accepts_temperature: true,
            prompt_cache: PromptCache::Implicit,
            // Function calls arrive as one chunk.
            streaming_tool_args: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let body = self.build_body(req)?;
        let url = self.gen_url(&req.model, "generateContent")?;
        let (v, rid) = self.http.post_json(url, &body).await?;
        parse_response(&v, rid)
    }

    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        let body = self.build_body(req)?;
        let mut url = self.gen_url(&req.model, "streamGenerateContent")?;
        // UNVERIFIED: `alt=sse` selects server-sent events for streamGenerateContent.
        url.query_pairs_mut().append_pair("alt", "sse");
        let resp = self.http.post_sse(url, &body).await?;
        Ok(sse_stream(resp, GeminiStream::default()))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        let mut url = self.http.url(&["models"])?;
        url.query_pairs_mut().append_pair("pageSize", "1");
        self.http.get_ok(url).await
    }
}

#[async_trait]
impl EmbeddingProvider for GeminiProvider {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        let m = model_segment(model);
        let requests: Vec<Value> = inputs
            .iter()
            .map(|t| json!({"model": format!("models/{m}"), "content": {"parts": [{"text": t}]}}))
            .collect();
        let url = self.gen_url(model, "batchEmbedContents")?;
        let (v, _) = self
            .http
            .post_json(url, &json!({"requests": requests}))
            .await?;
        let vectors: Vec<Vec<f32>> = v
            .get("embeddings")
            .and_then(Value::as_array)
            .ok_or_else(|| LlmError::Decode("missing embeddings".into()))?
            .iter()
            .map(|e| {
                e.get("values")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                    .unwrap_or_default()
            })
            .collect();
        Ok(Embeddings {
            dims: vectors.first().map_or(0, Vec::len),
            vectors,
            // UNVERIFIED: batchEmbedContents reports no token usage.
            usage: Usage::default(),
        })
    }
}
