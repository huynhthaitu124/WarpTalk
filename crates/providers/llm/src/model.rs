//! Provider-neutral request / response model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        /// Arguments as a JSON document (string form, as models stream it).
        args_json: String,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    /// A block the provider requires to be sent back unchanged (thinking + signature,
    /// reasoning items, thought signatures). Persist it with the message.
    Opaque {
        provider: ProviderId,
        blob: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text(text.into())],
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self::text(Role::User, text)
    }
    pub fn assistant(parts: Vec<Part>) -> Self {
        Self {
            role: Role::Assistant,
            parts,
        }
    }
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            role: Role::Tool,
            parts: vec![Part::ToolResult {
                id: id.into(),
                content: content.into(),
                is_error,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub json_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSchemaFormat {
    pub name: String,
    pub schema: Value,
    pub strict: bool,
}

/// Marks the stable prefix (tools, system, first `stable_prefix_messages` messages) as cacheable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CacheHint {
    pub stable_prefix_messages: usize,
    /// Used as `prompt_cache_key` where supported (e.g. workspace id).
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    pub response_format: Option<JsonSchemaFormat>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub cache_hint: Option<CacheHint>,
    /// Free-form tags (never prompt content). Only `user_id` is forwarded (Anthropic).
    pub metadata: BTreeMap<String, String>,
}

impl Request {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            response_format: None,
            max_output_tokens: None,
            temperature: None,
            cache_hint: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    End,
    ToolUse,
    MaxTokens,
    Filtered,
    Other,
}

/// Normalized token usage. `input_uncached + cache_read + cache_write` is the total input.
/// `output` is the billed output total; `reasoning` is the (informational) subset spent on
/// reasoning when the provider reports it separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_uncached: u32,
    pub cache_read: u32,
    pub cache_write: u32,
    pub output: u32,
    pub reasoning: u32,
}

impl Usage {
    pub fn total_input(&self) -> u32 {
        self.input_uncached
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
    pub fn total(&self) -> u32 {
        self.total_input().saturating_add(self.output)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub parts: Vec<Part>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub provider_request_id: Option<String>,
}

impl Response {
    /// Concatenated text parts.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    /// First delta of a call carries `id` and `name`; later ones only `args_delta`.
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        args_delta: String,
    },
    Opaque {
        provider: ProviderId,
        blob: Value,
    },
    Usage(Usage),
    Done {
        stop_reason: StopReason,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,
    pub dims: usize,
    pub usage: Usage,
}
