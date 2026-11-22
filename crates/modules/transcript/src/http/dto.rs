//! Request/response DTOs (camelCase JSON).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::service::TranscriptDetail;
use crate::domain::{Correction, SegmentView, Transcript};

#[derive(Debug, Deserialize)]
pub struct LangQuery {
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionsQuery {
    pub segment_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionRequest {
    pub segment_id: Uuid,
    pub text: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptDto {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub translation_room_id: Uuid,
    pub version: i32,
    pub status: &'static str,
    pub source_language: String,
    pub total_segments: i32,
    pub total_duration_ms: i32,
    pub is_current: bool,
    pub finalized_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub segments: Vec<SegmentDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentDto {
    pub id: Uuid,
    pub speaker_participant_id: Option<Uuid>,
    pub speaker_name: String,
    pub original_text: String,
    pub original_language: String,
    pub start_time_ms: Option<i32>,
    pub end_time_ms: Option<i32>,
    pub confidence: Option<f64>,
    pub sequence_order: i32,
    pub is_corrected: bool,
    pub is_final: bool,
    pub translation: Option<TranslationDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationDto {
    pub language: String,
    pub text: String,
    pub is_stale: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionDto {
    pub id: Uuid,
    pub segment_id: Uuid,
    pub user_id: Uuid,
    pub original_text: String,
    pub corrected_text: String,
    pub correction_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<SegmentView> for SegmentDto {
    fn from(v: SegmentView) -> Self {
        let s = v.segment;
        Self {
            id: s.id,
            speaker_participant_id: s.speaker_participant_id,
            speaker_name: s.speaker_name,
            original_text: s.original_text,
            original_language: s.original_language,
            start_time_ms: s.start_time_ms,
            end_time_ms: s.end_time_ms,
            confidence: s.confidence,
            sequence_order: s.sequence_order,
            is_corrected: s.is_corrected,
            is_final: s.is_final,
            translation: v.translation.map(|t| TranslationDto {
                language: t.language,
                text: t.text,
                is_stale: t.is_stale,
            }),
        }
    }
}

impl From<TranscriptDetail> for TranscriptDto {
    fn from(d: TranscriptDetail) -> Self {
        let Transcript {
            id,
            workspace_id,
            translation_room_id,
            version,
            status,
            source_language,
            total_segments,
            total_duration_ms,
            is_current,
            finalized_at,
            created_at,
            ..
        } = d.transcript;
        Self {
            id: id.as_uuid(),
            workspace_id: workspace_id.as_uuid(),
            translation_room_id: translation_room_id.as_uuid(),
            version,
            status: status.as_str(),
            source_language,
            total_segments,
            total_duration_ms,
            is_current,
            finalized_at,
            created_at,
            segments: d.segments.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Correction> for CorrectionDto {
    fn from(c: Correction) -> Self {
        Self {
            id: c.id,
            segment_id: c.segment_id,
            user_id: c.user_id.as_uuid(),
            original_text: c.original_text,
            corrected_text: c.corrected_text,
            correction_type: c.correction_type,
            status: c.status,
            reason: c.reason,
            created_at: c.created_at,
        }
    }
}
