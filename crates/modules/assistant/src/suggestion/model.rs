//! Value types of the suggestion pipeline and its delivery contract (spec 09 sections 9 and 7).

use serde::{Deserialize, Serialize};
use warptalk_kernel::{RoomId, WorkspaceId};

use super::config::ActionSpec;

/// Stream receiving suggestion results (`type = "suggestion"`), plus `:{meeting}` per-meeting copy.
pub const AI_ASSISTANT_RESULTS: &str = "ai_assistant:results";
/// Stream receiving structured follow-ups / action items for the room module to persist later.
pub const ASSISTANT_FOLLOWUPS: &str = "assistant:followups";
/// Consumer group of the suggestion worker on `stt:results` (spec 09 section 14).
pub const SUGGESTION_GROUP: &str = "suggestion-workers";

/// One transcript turn held in a room's rolling window.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub segment_id: String,
    pub speaker_id: String,
    pub text: String,
    pub language: String,
    /// Epoch milliseconds at which the turn entered the window (from the clock).
    pub at_ms: i64,
    pub tokens: u32,
}

/// Structured action item (spec 09 sections 7 and 15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub task: String,
    /// Deadline as spoken (verbatim), never parsed or invented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub source_segment_ids: Vec<String>,
}

/// Entry published on [`ASSISTANT_FOLLOWUPS`] (field `data`, JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUpEvent {
    pub suggestion_id: String,
    pub meeting_id: RoomId,
    pub workspace_id: WorkspaceId,
    pub segment_id: String,
    pub category: String,
    pub follow_up: FollowUp,
    pub confidence: f32,
    pub language: String,
    pub timestamp_ms: i64,
}

/// One citation chip (`sources_json` element).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub marker: String,
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
}

/// Wire message on [`AI_ASSISTANT_RESULTS`]. Published as flat string fields (the original
/// contract) plus `data` holding this struct as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionResult {
    pub suggestion_id: String,
    pub meeting_id: RoomId,
    pub workspace_id: WorkspaceId,
    pub segment_id: String,
    /// Always `"suggestion"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Wire category (rule `wire_name`, defaults to the category id).
    pub category: String,
    pub severity: String,
    pub content: String,
    pub detail: String,
    pub confidence: f32,
    pub language: String,
    pub token_count: u32,
    pub sources: Vec<SourceRef>,
    pub timestamp_ms: i64,
    pub ttl_seconds: u32,
    /// The hint states a contradiction with a retrieved source.
    pub conflict: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<FollowUp>,
    pub actions: Vec<ActionSpec>,
    pub prompt_version: String,
}

impl SuggestionResult {
    /// `sources_json` as the chip array every AI surface renders (empty string when none).
    pub fn sources_json(&self) -> String {
        if self.sources.is_empty() {
            return String::new();
        }
        serde_json::to_string(&self.sources).unwrap_or_default()
    }

    /// Flat stream fields (spec 09 section 9) plus `data`.
    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut f = vec![
            ("meeting_id", self.meeting_id.to_string()),
            ("workspace_id", self.workspace_id.to_string()),
            ("segment_id", self.segment_id.clone()),
            ("suggestion_id", self.suggestion_id.clone()),
            ("type", self.kind.clone()),
            ("category", self.category.clone()),
            ("severity", self.severity.clone()),
            ("content", self.content.clone()),
            ("detail", self.detail.clone()),
            ("confidence", self.confidence.to_string()),
            ("language", self.language.clone()),
            ("token_count", self.token_count.to_string()),
            ("sources_json", self.sources_json()),
            ("timestamp_ms", self.timestamp_ms.to_string()),
            ("ttl_seconds", self.ttl_seconds.to_string()),
            ("conflict", self.conflict.to_string()),
            ("prompt_version", self.prompt_version.clone()),
        ];
        if let Some(fu) = &self.follow_up {
            f.push(("follow_up_json", serde_json::to_string(fu).unwrap_or_default()));
        }
        if !self.actions.is_empty() {
            f.push((
                "actions_json",
                serde_json::to_string(&self.actions).unwrap_or_default(),
            ));
        }
        f.push(("data", serde_json::to_string(self).unwrap_or_default()));
        f
    }
}

/// Why a segment ended without a published suggestion, or that it was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Control marker or empty segment.
    Ignored,
    Disabled,
    RoomInactive,
    FlagOff,
    TranscriptPaused,
    Stage0Rejected(&'static str),
    CapZero,
    /// External provider but the workspace did not consent (or no provider could be resolved).
    BlockedPolicy,
    ProviderUnavailable,
    CooldownActive,
    BudgetExhausted,
    /// A pipeline for this room is already running (bounded concurrency 1); stage 1 skipped.
    Busy,
    DecideDeclined,
    DecideFailed,
    BelowConfidence,
    UnknownCategory,
    CategoryDisabled,
    /// "Fact needs docs": nothing grounded was retrieved.
    NeedsDocuments,
    SlotDenied,
    GenerateEmpty,
    GenerateFailed,
    HygieneDropped,
    Duplicate,
    PublishFailed,
    Published,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Disabled => "disabled",
            Self::RoomInactive => "room_inactive",
            Self::FlagOff => "flag_off",
            Self::TranscriptPaused => "transcript_paused",
            Self::Stage0Rejected(_) => "stage0_rejected",
            Self::CapZero => "cap_zero",
            Self::BlockedPolicy => "blocked_policy",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::CooldownActive => "cooldown_active",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Busy => "busy",
            Self::DecideDeclined => "decide_declined",
            Self::DecideFailed => "decide_failed",
            Self::BelowConfidence => "below_confidence",
            Self::UnknownCategory => "unknown_category",
            Self::CategoryDisabled => "category_disabled",
            Self::NeedsDocuments => "needs_documents",
            Self::SlotDenied => "slot_denied",
            Self::GenerateEmpty => "generate_empty",
            Self::GenerateFailed => "generate_failed",
            Self::HygieneDropped => "hygiene_dropped",
            Self::Duplicate => "duplicate",
            Self::PublishFailed => "publish_failed",
            Self::Published => "published",
        }
    }

    pub fn is_published(&self) -> bool {
        matches!(self, Self::Published)
    }
}
