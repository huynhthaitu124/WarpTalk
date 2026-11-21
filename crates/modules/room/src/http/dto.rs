use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_contracts::events::AudioRoutesSnapshot;
use warptalk_kernel::{Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId};

use crate::application::ports::{ClaimOutcome, ParticipantRecord, RoomRecord};
use crate::domain::{BridgeConfig, BridgeState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRoomRequest {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub display_name: Option<String>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub requires_approval: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinByCodeRequest {
    pub room_code: String,
    pub display_name: Option<String>,
    pub speak_language: String,
    pub listen_language: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinByIdRequest {
    pub display_name: Option<String>,
    pub speak_language: String,
    pub listen_language: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub workspace_id: WorkspaceId,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub status: Option<String>,
}

impl ListQuery {
    pub fn page_request(&self) -> PageRequest {
        let d = PageRequest::default();
        PageRequest {
            page: self.page.unwrap_or(d.page),
            page_size: self.page_size.unwrap_or(d.page_size),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomDto {
    pub id: RoomId,
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    pub active_host_id: Option<UserId>,
    pub effective_host_id: UserId,
    pub title: String,
    pub room_code: String,
    pub status: &'static str,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub requires_approval: bool,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub translation_active: bool,
    /// `NATIVE` or `EXTERNAL_BRIDGE`.
    pub room_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge: Option<BridgeDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDto {
    pub provider: &'static str,
    pub meeting_code: String,
    pub join_url: Option<String>,
    pub capturer_user_id: Option<UserId>,
    pub capturer_lease_at: Option<DateTime<Utc>>,
    pub audio_flowing: Option<bool>,
    pub last_audio_at: Option<DateTime<Utc>>,
    pub audio_mode: &'static str,
    pub epoch: i32,
}

impl From<BridgeState> for BridgeDto {
    fn from(b: BridgeState) -> Self {
        Self {
            provider: b.provider.as_str(),
            meeting_code: b.meeting_code,
            join_url: b.join_url,
            capturer_user_id: b.lease.user_id.map(Into::into),
            capturer_lease_at: b.lease.lease_at,
            audio_flowing: b.lease.audio_flowing,
            last_audio_at: b.lease.last_audio_at,
            audio_mode: b.audio_mode.as_str(),
            epoch: b.epoch,
        }
    }
}

impl From<RoomRecord> for RoomDto {
    fn from(r: RoomRecord) -> Self {
        Self {
            effective_host_id: r.effective_host(),
            id: r.id,
            workspace_id: r.workspace_id,
            host_id: r.host_id,
            active_host_id: r.active_host_id,
            title: r.title,
            room_code: r.code,
            status: r.status.as_str(),
            source_language: r.source_language,
            target_languages: r.target_languages,
            requires_approval: r.requires_approval,
            scheduled_at: r.scheduled_at,
            started_at: r.started_at,
            ended_at: r.ended_at,
            created_at: r.created_at,
            translation_active: r.translation_active,
            room_type: r.room_type.as_str(),
            bridge: r.bridge.map(Into::into),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimBridgeRequest {
    pub workspace_id: WorkspaceId,
    pub provider: String,
    pub meeting_url: Option<String>,
    pub code: Option<String>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub source_language: String,
    pub target_languages: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRequest {
    /// Defaults to `true` for clients that do not report audio liveness.
    pub audio_flowing: Option<bool>,
    pub last_audio_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct AudioModeRequest {
    pub mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimBridgeResponse {
    #[serde(flatten)]
    pub room: RoomDto,
    pub created: bool,
    /// `capturer` or `viewer`.
    pub bridge_role: &'static str,
    pub heartbeat_interval_seconds: u64,
    pub lease_seconds: u64,
}

impl ClaimBridgeResponse {
    pub fn new(o: ClaimOutcome, cfg: BridgeConfig) -> Self {
        Self {
            room: o.room.into(),
            created: o.created,
            bridge_role: if o.is_capturer { "capturer" } else { "viewer" },
            heartbeat_interval_seconds: cfg.heartbeat_interval.as_secs(),
            lease_seconds: cfg.stale_after.as_secs(),
        }
    }
}

/// Capturer state after heartbeat/takeover/audio-mode.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStateResponse {
    pub room_id: RoomId,
    #[serde(flatten)]
    pub bridge: BridgeDto,
    pub heartbeat_interval_seconds: u64,
    pub lease_seconds: u64,
}

impl BridgeStateResponse {
    pub fn new(r: RoomRecord, cfg: BridgeConfig) -> Option<Self> {
        Some(Self {
            room_id: r.id,
            bridge: r.bridge?.into(),
            heartbeat_interval_seconds: cfg.heartbeat_interval.as_secs(),
            lease_seconds: cfg.stale_after.as_secs(),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDto<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

impl<T> PageDto<T> {
    pub fn map<S>(p: Page<S>, f: impl FnMut(S) -> T) -> Self {
        Self {
            items: p.items.into_iter().map(f).collect(),
            page: p.page,
            page_size: p.page_size,
            total: p.total,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantDto {
    pub id: ParticipantId,
    pub user_id: Option<UserId>,
    pub display_name: String,
    pub role: &'static str,
    pub speak_language: String,
    pub listen_language: String,
    pub status: &'static str,
    pub is_using_voice_clone: bool,
    pub joined_at: Option<DateTime<Utc>>,
    pub left_at: Option<DateTime<Utc>>,
}

impl From<ParticipantRecord> for ParticipantDto {
    fn from(p: ParticipantRecord) -> Self {
        Self {
            id: p.id,
            user_id: p.user_id,
            display_name: p.display_name,
            role: if p.is_host { "HOST" } else { "PARTICIPANT" },
            speak_language: p.speak_language,
            listen_language: p.listen_language,
            status: p.status.as_str(),
            is_using_voice_clone: p.is_using_voice_clone,
            joined_at: p.joined_at,
            left_at: p.left_at,
        }
    }
}

/// Join response: the participant fields (flat, as before) plus `roomId` and a room summary.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinedDto {
    pub room_id: RoomId,
    pub room: RoomSummaryDto,
    #[serde(flatten)]
    pub participant: ParticipantDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummaryDto {
    pub title: String,
    pub status: &'static str,
    pub room_type: &'static str,
    pub source_language: String,
    pub target_languages: Vec<String>,
}

impl From<(RoomRecord, ParticipantRecord)> for JoinedDto {
    fn from((r, p): (RoomRecord, ParticipantRecord)) -> Self {
        Self {
            room_id: r.id,
            room: RoomSummaryDto {
                title: r.title,
                status: r.status.as_str(),
                room_type: r.room_type.as_str(),
                source_language: r.source_language,
                target_languages: r.target_languages,
            },
            participant: p.into(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDto {
    pub route_id: Uuid,
    pub source_participant_id: ParticipantId,
    pub target_participant_id: ParticipantId,
    pub source_language: String,
    pub target_language: String,
    pub voice_clone_enabled: bool,
    pub status: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutesDto {
    pub generated_at: DateTime<Utc>,
    pub room_status: String,
    pub translation_active: bool,
    pub room_languages: Vec<String>,
    pub routes: Vec<RouteDto>,
}

impl From<AudioRoutesSnapshot> for RoutesDto {
    fn from(s: AudioRoutesSnapshot) -> Self {
        Self {
            generated_at: s.generated_at,
            room_status: s.room_status,
            translation_active: s.translation_active,
            room_languages: s.room_languages,
            routes: s
                .routes
                .into_iter()
                .map(|r| RouteDto {
                    route_id: r.route_id,
                    source_participant_id: r.source_participant_id,
                    target_participant_id: r.target_participant_id,
                    source_language: r.source_language,
                    target_language: r.target_language,
                    voice_clone_enabled: r.voice_clone_enabled,
                    status: serde_json::to_value(r.status).unwrap_or_default(),
                })
                .collect(),
        }
    }
}
