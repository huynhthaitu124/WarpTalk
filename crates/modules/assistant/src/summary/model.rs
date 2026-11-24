//! Wire and stored shapes of the summary feature.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{RoomId, WorkspaceId};

/// Consumer group of the summary worker on `stt:results` and `assistant:summary_requests`.
pub const SUMMARY_GROUP: &str = "meeting-summary";

/// Error codes stored in `meeting_summaries.error_code` (the UI shows them).
pub mod codes {
    pub const LLM_NOT_CONFIGURED: &str = "LLM_NOT_CONFIGURED";
    pub const LLM_UNAVAILABLE: &str = "LLM_UNAVAILABLE";
    pub const LLM_FAILED: &str = "LLM_FAILED";
    pub const LLM_BAD_OUTPUT: &str = "LLM_BAD_OUTPUT";
    pub const NO_TRANSCRIPT: &str = "NO_TRANSCRIPT";
    pub const ROOM_NOT_FOUND: &str = "ROOM_NOT_FOUND";
    pub const INTERNAL: &str = "INTERNAL";
    pub const ABANDONED: &str = "ABANDONED";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl SummaryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Auto,
    Manual,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// One `meeting_summaries` row.
#[derive(Debug, Clone)]
pub struct SummaryJob {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub room_id: RoomId,
    pub version: i32,
    pub transcript_version: i32,
    pub trigger: String,
    pub status: SummaryStatus,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub content: Option<SummaryContent>,
    pub error_code: Option<String>,
    pub error: Option<String>,
    pub attempts: i32,
    pub requested_by: Option<Uuid>,
    pub input_segments: Option<i32>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Action item as stored inside `content` (and as validated model output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItemDraft {
    pub task: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub source_segment_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummaryMeta {
    /// Segments the model saw.
    pub input_segments: u32,
    /// Segments in the transcript (more than `input_segments` when capped).
    pub total_segments: u32,
    pub truncated: bool,
    /// Map windows (1 = single pass).
    pub windows: u32,
    pub llm_calls: u32,
    pub live_followups_used: u32,
}

/// The stored summary document (`content` column).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummaryContent {
    pub summary_short: String,
    pub summary_detailed: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItemDraft>,
    pub open_questions: Vec<String>,
    pub topics: Vec<String>,
    pub language: String,
    pub meta: SummaryMeta,
}

/// One `action_items` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionItem {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub room_id: RoomId,
    pub summary_id: Uuid,
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub status: String,
    pub source_segment_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Body of `assistant:summary_requests` (field `data`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRequest {
    pub summary_id: Uuid,
    pub room_id: RoomId,
    pub workspace_id: WorkspaceId,
}

/// Body of `assistant:summary_results` (field `data`), published when a job reaches a final state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryResultEvent {
    pub summary_id: Uuid,
    pub room_id: RoomId,
    pub workspace_id: WorkspaceId,
    pub version: i32,
    pub status: SummaryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Structured-output schema (strict: every property required, optional ones nullable).
pub fn output_schema() -> Value {
    let str_list = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "object",
        "properties": {
            "summary_short": {"type": "string"},
            "summary_detailed": {"type": "string"},
            "decisions": str_list,
            "action_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": {"type": "string"},
                        "owner": {"type": ["string", "null"]},
                        "due": {"type": ["string", "null"]},
                        "source_segments": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["task", "owner", "due", "source_segments"],
                    "additionalProperties": false
                }
            },
            "open_questions": str_list,
            "topics": str_list
        },
        "required": ["summary_short", "summary_detailed", "decisions", "action_items",
                     "open_questions", "topics"],
        "additionalProperties": false
    })
}

/// Raw model reply (before validation).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawOutput {
    pub summary_short: String,
    pub summary_detailed: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<RawActionItem>,
    pub open_questions: Vec<String>,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawActionItem {
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub source_segments: Vec<String>,
}
