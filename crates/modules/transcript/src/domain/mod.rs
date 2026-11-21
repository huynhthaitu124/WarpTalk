//! Entities, value objects, state machines. Pure Rust, no IO.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, RoomId, TranscriptId, UserId, WorkspaceId};

/// Transcript lifecycle (spec 12 §4.1). Ingestion creates `Recording`;
/// finalize writes `Finalized`. `Finalizing` is the accepted intermediate state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptStatus {
    Recording,
    Finalizing,
    Finalized,
    Archived,
}

/// What a finalize request has to do for a given status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeAction {
    /// Move to `FINALIZED`.
    Proceed,
    /// Already finalized: success no-op.
    AlreadyDone,
}

impl TranscriptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "RECORDING",
            Self::Finalizing => "FINALIZING",
            Self::Finalized => "FINALIZED",
            Self::Archived => "ARCHIVED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "RECORDING" => Some(Self::Recording),
            "FINALIZING" => Some(Self::Finalizing),
            "FINALIZED" => Some(Self::Finalized),
            "ARCHIVED" => Some(Self::Archived),
            _ => None,
        }
    }

    /// FINALIZED and ARCHIVED transcripts refuse corrections (spec 12 §5.2).
    pub fn is_locked(self) -> bool {
        matches!(self, Self::Finalized | Self::Archived)
    }

    pub fn finalize(self) -> Result<FinalizeAction, AppError> {
        match self {
            Self::Recording | Self::Finalizing => Ok(FinalizeAction::Proceed),
            Self::Finalized => Ok(FinalizeAction::AlreadyDone),
            Self::Archived => Err(AppError::validation(
                "TRANSCRIPT_ARCHIVED",
                "an archived transcript cannot be finalized",
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transcript {
    pub id: TranscriptId,
    pub workspace_id: WorkspaceId,
    pub translation_room_id: RoomId,
    pub version: i32,
    pub status: TranscriptStatus,
    pub source_language: String,
    pub total_segments: i32,
    pub total_duration_ms: i32,
    pub is_current: bool,
    pub previous_transcript_id: Option<TranscriptId>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub id: Uuid,
    pub transcript_id: TranscriptId,
    pub speaker_participant_id: Option<Uuid>,
    pub speaker_name: String,
    pub original_text: String,
    pub original_language: String,
    pub start_time_ms: i32,
    pub end_time_ms: i32,
    pub confidence: Option<f64>,
    pub sequence_order: i32,
    pub is_corrected: bool,
    pub is_final: bool,
}

/// Segment plus its current translation for one language.
#[derive(Debug, Clone)]
pub struct SegmentView {
    pub segment: Segment,
    pub translation: Option<SegmentTranslation>,
}

#[derive(Debug, Clone)]
pub struct SegmentTranslation {
    pub language: String,
    pub text: String,
    pub is_stale: bool,
}

#[derive(Debug, Clone)]
pub struct Correction {
    pub id: Uuid,
    pub segment_id: Uuid,
    pub user_id: UserId,
    pub original_text: String,
    pub corrected_text: String,
    pub correction_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Wire segment ids are opaque strings. Translation and TTS workers mint
/// `"{segmentUuid}-c{idx}"` per sentence chunk; recover the underlying UUID.
pub fn underlying_segment_id(raw: &str) -> Option<Uuid> {
    let head = match raw.get(..36) {
        Some(h) if raw.len() == 36 || raw.as_bytes().get(36) == Some(&b'-') => h,
        _ => return None,
    };
    Uuid::parse_str(head).ok()
}

/// Map the producer's voice type to the stored vocabulary (`cloned` | `default`).
pub fn normalize_voice_type(raw: Option<&str>) -> &'static str {
    match raw.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("cloned") | Some("clone") => "cloned",
        _ => "default",
    }
}

/// Truncate to at most `max` characters (not bytes).
pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn segment_id_suffix() {
        let id = Uuid::now_v7();
        assert_eq!(underlying_segment_id(&id.to_string()), Some(id));
        assert_eq!(underlying_segment_id(&format!("{id}-c3")), Some(id));
        assert_eq!(underlying_segment_id(&format!("{id}x")), None);
        assert_eq!(underlying_segment_id("nope"), None);
        assert_eq!(underlying_segment_id("é"), None);
    }

    #[test]
    fn finalize_transitions() {
        assert_eq!(
            TranscriptStatus::Recording.finalize().unwrap(),
            FinalizeAction::Proceed
        );
        assert_eq!(
            TranscriptStatus::Finalized.finalize().unwrap(),
            FinalizeAction::AlreadyDone
        );
        assert!(TranscriptStatus::Archived.finalize().is_err());
        assert!(TranscriptStatus::Finalized.is_locked());
        assert!(!TranscriptStatus::Finalizing.is_locked());
    }

    #[test]
    fn voice_types() {
        assert_eq!(normalize_voice_type(Some("clone")), "cloned");
        assert_eq!(normalize_voice_type(None), "default");
    }
}
