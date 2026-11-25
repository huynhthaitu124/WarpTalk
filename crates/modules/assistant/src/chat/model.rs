//! Wire shapes of the ask / recap / history API.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod codes {
    pub const LLM_NOT_CONFIGURED: &str = "LLM_NOT_CONFIGURED";
    pub const LLM_UNAVAILABLE: &str = "LLM_UNAVAILABLE";
    pub const LLM_FAILED: &str = "LLM_FAILED";
    pub const ANSWER_BLOCKED: &str = "ANSWER_BLOCKED";
    pub const LLM_EMPTY: &str = "LLM_EMPTY";
    pub const EXTERNAL_LLM_NOT_ALLOWED: &str = "EXTERNAL_LLM_NOT_ALLOWED";
    pub const CHAT_DISABLED: &str = "ASSISTANT_CHAT_DISABLED";
    pub const QUESTION_EMPTY: &str = "QUESTION_EMPTY";
    pub const QUESTION_TOO_LONG: &str = "QUESTION_TOO_LONG";
    pub const INVALID_WINDOW: &str = "INVALID_RECAP_WINDOW";
    pub const INVALID_SINCE: &str = "INVALID_SINCE_MS";
}

/// What a question may draw on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChatScope {
    /// This meeting's transcript (and earlier-meeting context).
    #[serde(rename = "meeting")]
    Meeting,
    /// Plus workspace documents (default).
    #[default]
    #[serde(rename = "meeting+docs")]
    MeetingDocs,
    /// Plus a wider document search over the whole workspace.
    #[serde(rename = "workspace")]
    Workspace,
}

impl ChatScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meeting => "meeting",
            Self::MeetingDocs => "meeting+docs",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    pub question: String,
    #[serde(default)]
    pub scope: Option<ChatScope>,
    /// Only transcript from this meeting offset (ms) on.
    #[serde(default, alias = "sinceMs")]
    pub since_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecapRequest {
    /// 5, 10, 15 (configurable) or `null` for the whole meeting.
    #[serde(default, alias = "lastMinutes")]
    pub last_minutes: Option<u32>,
    /// Output language (ISO code or name); default: the room's source language.
    #[serde(default)]
    pub language: Option<String>,
}

/// A source the answer relies on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// `transcript` | `document`.
    pub kind: String,
    /// Transcript segment id, or document chunk id.
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub label: String,
    /// Transcript offset in ms (transcript citations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

/// Non-streaming response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAnswer {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub model: String,
    pub message_id: Option<Uuid>,
    /// Served from the recap cache (no LLM call).
    pub cached: bool,
}

/// What the service emits while answering (the SSE events).
#[derive(Debug, Clone)]
pub enum ChatEvent {
    Delta(String),
    Citations(Vec<Citation>),
    Done {
        model: String,
        message_id: Option<Uuid>,
        cached: bool,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: Uuid,
    /// `user` | `assistant`.
    pub role: String,
    /// `ask` | `recap`.
    pub kind: String,
    pub content: String,
    pub citations: Vec<Citation>,
    pub model: Option<String>,
    pub scope: Option<String>,
    pub created_at: DateTime<Utc>,
}
