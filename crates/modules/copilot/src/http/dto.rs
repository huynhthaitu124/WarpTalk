//! Request/response bodies (camelCase JSON).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::application::ports::SttToken;
use crate::application::service::{CreateSession, IngestOutcome, SessionView};
use crate::domain::{Channel, Segment};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguagesBody {
    /// Language of the user's own microphone channel.
    pub me: String,
    /// Language of the other participants.
    pub others: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayNamesBody {
    pub me: Option<String>,
    pub others: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub workspace_id: Uuid,
    pub title: Option<String>,
    pub platform: String,
    pub meeting_url: Option<String>,
    pub languages: LanguagesBody,
    #[serde(default)]
    pub display_names: DisplayNamesBody,
}

impl From<CreateSessionRequest> for CreateSession {
    fn from(r: CreateSessionRequest) -> Self {
        Self {
            workspace_id: r.workspace_id.into(),
            title: r.title,
            platform: r.platform,
            meeting_url: r.meeting_url,
            language_me: r.languages.me,
            language_others: r.languages.others,
            name_me: r.display_names.me,
            name_others: r.display_names.others,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default, alias = "page_size")]
    pub page_size: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguagesDto {
    pub me: String,
    pub others: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    /// Same as `roomId`: use it for `/transcripts/{id}` and `/meetings/{id}/suggestions/stream`.
    pub id: Uuid,
    pub room_id: Uuid,
    pub workspace_id: Uuid,
    pub host_id: Uuid,
    pub title: String,
    pub platform: &'static str,
    pub meeting_url: Option<String>,
    pub languages: LanguagesDto,
    pub display_names: LanguagesDto,
    /// `IN_PROGRESS` while running, `ENDED` afterwards.
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<SessionView> for SessionDto {
    fn from(v: SessionView) -> Self {
        let r = v.record;
        Self {
            id: r.room_id.as_uuid(),
            room_id: r.room_id.as_uuid(),
            workspace_id: r.workspace_id.as_uuid(),
            host_id: r.host_id.as_uuid(),
            title: r.title,
            platform: r.platform.as_str(),
            meeting_url: r.meeting_url,
            languages: LanguagesDto {
                me: r.language_me,
                others: r.language_others,
            },
            display_names: LanguagesDto {
                me: r.name_me,
                others: r.name_others,
            },
            status: v.status,
            started_at: v.started_at,
            ended_at: v.ended_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentBody {
    pub client_segment_id: String,
    /// `me` | `others`.
    pub channel: String,
    pub text: String,
    pub language: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub confidence: Option<f64>,
    pub is_final: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestRequest {
    pub segments: Vec<SegmentBody>,
}

impl SegmentBody {
    /// `None` when the channel is unknown.
    pub fn into_segment(self) -> Option<Segment> {
        Some(Segment {
            channel: Channel::parse(&self.channel)?,
            client_segment_id: self.client_segment_id,
            text: self.text,
            language: self.language,
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            confidence: self.confidence,
            is_final: self.is_final,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestedSegment {
    pub client_segment_id: String,
    /// Transcript segment id (`GET /transcripts/{roomId}`).
    pub segment_id: Uuid,
    pub duplicate: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestResponse {
    pub accepted: usize,
    pub duplicates: usize,
    pub segments: Vec<IngestedSegment>,
    /// `true` when the workspace has no AI credits left: the segments were accepted and kept, but
    /// suggestions and STT tokens are suspended until credits are added.
    pub insufficient_credits: bool,
}

impl From<IngestOutcome> for IngestResponse {
    fn from(o: IngestOutcome) -> Self {
        Self {
            accepted: o.accepted,
            duplicates: o.duplicates,
            insufficient_credits: o.insufficient_credits,
            segments: o
                .segments
                .into_iter()
                .map(
                    |(client_segment_id, segment_id, duplicate)| IngestedSegment {
                        client_segment_id,
                        segment_id,
                        duplicate,
                    },
                )
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttTokenResponse {
    pub provider: String,
    pub model: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub websocket_url: String,
    pub session_config: Value,
}

impl From<SttToken> for SttTokenResponse {
    fn from(t: SttToken) -> Self {
        Self {
            provider: t.provider,
            model: t.model,
            token: t.token,
            expires_at: t.expires_at,
            websocket_url: t.websocket_url,
            session_config: t.session_config,
        }
    }
}
