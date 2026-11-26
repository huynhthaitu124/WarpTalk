//! WarpBot conversations: entities and the pure rules around them (titles, citations).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{UserId, WorkspaceId};

/// Longest prompt a user may send in one message (characters).
pub const MAX_PROMPT_CHARS: usize = 8_000;
/// Longest derived conversation title (characters, before the ellipsis).
pub const MAX_TITLE_CHARS: usize = 60;
pub const DEFAULT_TITLE: &str = "New chat";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

impl MessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "system" => Some(Self::System),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    Pending,
    Streaming,
    Completed,
    Failed,
}

impl MessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "streaming" => Some(Self::Streaming),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Still being written (no final content yet).
    pub fn in_flight(self) -> bool {
        matches!(self, Self::Pending | Self::Streaming)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub workspace_id: WorkspaceId,
    /// `None` on the assistant's own replies.
    pub user_id: Option<UserId>,
    pub role: MessageRole,
    pub content: String,
    pub status: MessageStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Cited sources of an assistant reply (JSON array of [`AnswerSource`]).
    pub sources_json: Option<String>,
}

/// One tool call of a reply, as recorded in `assistant_tool_calls`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    pub id: Uuid,
    pub message_id: Uuid,
    pub tool_name: String,
    pub arguments_json: String,
    pub created_at: DateTime<Utc>,
}

/// A source shown to the model under a marker (`S1`, `S2`, ...). The reply keeps only the ones it
/// cites. Field names are the web client's `AnswerSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerSource {
    pub marker: String,
    /// document | transcript | meeting | glossary | knowledge | web
    pub kind: String,
    pub title: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub ref_id: Option<String>,
}

/// Conversation title from its first prompt: first non-empty line, whitespace collapsed, cut on a
/// character boundary.
pub fn derive_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return DEFAULT_TITLE.to_owned();
    }
    if collapsed.chars().count() <= MAX_TITLE_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(MAX_TITLE_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// True when `text` contains `marker` as a whole token (`S1` matches "[S1]" and "S1," but not
/// "S10" or "XS1").
fn cites(text: &str, marker: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(pos) = text[from..].find(marker) {
        let start = from + pos;
        let end = start + marker.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// The offered sources the answer points at, in offer order. Markers the turn never issued
/// resolve to nothing, so an invented citation produces no chip.
pub fn cited_sources(answer: &str, offered: &[AnswerSource]) -> Vec<AnswerSource> {
    offered
        .iter()
        .filter(|s| cites(answer, &s.marker))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(marker: &str) -> AnswerSource {
        AnswerSource {
            marker: marker.to_owned(),
            kind: "document".to_owned(),
            title: format!("Doc {marker}"),
            ref_id: None,
        }
    }

    #[test]
    fn titles_come_from_the_first_line_and_are_cut() {
        assert_eq!(derive_title("\n  hello   world \nsecond"), "hello world");
        assert_eq!(derive_title("   "), DEFAULT_TITLE);
        let long = "a".repeat(100);
        let t = derive_title(&long);
        assert_eq!(t.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(t.ends_with('…'));
        // Multi-byte characters are cut on a boundary.
        let vi = "Tóm tắt cuộc họp ".repeat(10);
        assert!(derive_title(&vi).ends_with('…'));
    }

    #[test]
    fn only_cited_markers_are_kept() {
        let offered = vec![src("S1"), src("S2"), src("S10")];
        let kept = cited_sources("Per [S2] and S10, not S3.", &offered);
        assert_eq!(
            kept.iter().map(|s| s.marker.as_str()).collect::<Vec<_>>(),
            vec!["S2", "S10"]
        );
        assert!(cited_sources("XS1 S1x", &offered).is_empty());
    }

    #[test]
    fn sources_serialize_with_the_client_field_names() {
        let mut s = src("S1");
        s.ref_id = Some("abc".to_owned());
        let v = serde_json::to_value(&s).unwrap_or_default();
        assert_eq!(v["ref"], "abc");
        assert!(
            serde_json::to_value(src("S2"))
                .unwrap_or_default()
                .get("ref")
                .is_none()
        );
    }
}
