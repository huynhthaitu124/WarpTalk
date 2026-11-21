//! Ports: what this module needs from the outside (workspace policy, voice consent, event
//! publishing) plus the persistence port implemented in `infrastructure`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use warptalk_contracts::events::{AudioRoutesSnapshot, RoomCommand};
use warptalk_kernel::{
    AppError, AppResult, Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId,
    WorkspaceRole,
};

use crate::domain::{
    AudioMode, BridgeConfig, BridgeProvider, BridgeState, ParticipantStatus, RoomAction,
    RoomStatus, RoomType,
};

// ---------------------------------------------------------------- outbound ports

/// Workspace membership/availability (api wires the workspace module's `WorkspaceQueries`).
#[async_trait]
pub trait WorkspacePolicy: Send + Sync {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
    async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool>;
}

/// Denies everything: nobody is a member, no workspace is active.
pub struct NoopWorkspacePolicy;

#[async_trait]
impl WorkspacePolicy for NoopWorkspacePolicy {
    async fn member_role(&self, _: WorkspaceId, _: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(None)
    }
    async fn is_active(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(false)
    }
}

/// In-memory policy for tests: workspaces are active unless marked inactive.
#[derive(Default)]
pub struct InMemoryWorkspacePolicy {
    roles: Mutex<HashMap<(WorkspaceId, UserId), WorkspaceRole>>,
    inactive: Mutex<HashSet<WorkspaceId>>,
}

impl InMemoryWorkspacePolicy {
    pub fn set_role(&self, ws: WorkspaceId, user: UserId, role: WorkspaceRole) {
        self.roles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((ws, user), role);
    }

    pub fn set_active(&self, ws: WorkspaceId, active: bool) {
        let mut g = self.inactive.lock().unwrap_or_else(|e| e.into_inner());
        if active {
            g.remove(&ws);
        } else {
            g.insert(ws);
        }
    }
}

#[async_trait]
impl WorkspacePolicy for InMemoryWorkspacePolicy {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        let active = self.is_active(ws).await?;
        let role = self
            .roles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(ws, user))
            .copied();
        Ok(role.filter(|_| active))
    }
    async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool> {
        Ok(!self
            .inactive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&ws))
    }
}

/// Voice-clone consent lookup (api wires the identity/voice module).
#[async_trait]
pub trait VoiceConsentDirectory: Send + Sync {
    async fn has_voice_clone_consent(&self, user: UserId) -> AppResult<bool>;
}

/// Nobody has consented.
pub struct NoopVoiceConsentDirectory;

#[async_trait]
impl VoiceConsentDirectory for NoopVoiceConsentDirectory {
    async fn has_voice_clone_consent(&self, _: UserId) -> AppResult<bool> {
        Ok(false)
    }
}

/// Publishes pipeline-facing events. Called after the database transaction committed.
#[async_trait]
pub trait RoomEventPublisher: Send + Sync {
    /// Command on `TRANSLATION_ROOM_COMMANDS`.
    async fn publish_command(&self, cmd: &RoomCommand) -> AppResult<()>;
    /// Cache the snapshot under the `audio_routes` key (TTL 12 h) and announce AUDIO_ROUTES_UPDATED.
    async fn publish_routes(&self, room: RoomId, snapshot: &AudioRoutesSnapshot) -> AppResult<()>;
}

/// Publisher that drops everything.
pub struct NoopRoomEventPublisher;

#[async_trait]
impl RoomEventPublisher for NoopRoomEventPublisher {
    async fn publish_command(&self, _: &RoomCommand) -> AppResult<()> {
        Ok(())
    }
    async fn publish_routes(&self, _: RoomId, _: &AudioRoutesSnapshot) -> AppResult<()> {
        Ok(())
    }
}

/// Records everything published (tests). `fail_routes` makes `publish_routes` fail.
#[derive(Default)]
pub struct RecordingRoomEventPublisher {
    pub commands: Mutex<Vec<RoomCommand>>,
    pub routes: Mutex<Vec<(RoomId, AudioRoutesSnapshot)>>,
    pub fail_routes: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl RoomEventPublisher for RecordingRoomEventPublisher {
    async fn publish_command(&self, cmd: &RoomCommand) -> AppResult<()> {
        self.commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(cmd.clone());
        Ok(())
    }
    async fn publish_routes(&self, room: RoomId, snapshot: &AudioRoutesSnapshot) -> AppResult<()> {
        if self.fail_routes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AppError::unavailable("route publication failed"));
        }
        self.routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((room, snapshot.clone()));
        Ok(())
    }
}

// ---------------------------------------------------------------- persistence port

#[derive(Debug, Clone)]
pub struct RoomRecord {
    pub id: RoomId,
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    pub active_host_id: Option<UserId>,
    pub title: String,
    pub code: String,
    pub status: RoomStatus,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub requires_approval: bool,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// IN_PROGRESS with an ACTIVE session.
    pub translation_active: bool,
    pub room_type: RoomType,
    /// Present only for `EXTERNAL_BRIDGE` rooms.
    pub bridge: Option<BridgeState>,
}

impl RoomRecord {
    pub fn effective_host(&self) -> UserId {
        self.active_host_id.unwrap_or(self.host_id)
    }
}

#[derive(Debug, Clone)]
pub struct ParticipantRecord {
    pub id: ParticipantId,
    pub user_id: Option<UserId>,
    pub display_name: String,
    pub is_host: bool,
    pub speak_language: String,
    pub listen_language: String,
    pub status: ParticipantStatus,
    pub is_using_voice_clone: bool,
    pub joined_at: Option<DateTime<Utc>>,
    pub left_at: Option<DateTime<Utc>>,
}

pub struct NewRoom {
    pub id: RoomId,
    pub workspace_id: WorkspaceId,
    pub host: UserId,
    pub host_display_name: String,
    pub title: String,
    pub code: String,
    pub status: RoomStatus,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub requires_approval: bool,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
}

pub struct NewBridgeRoom {
    pub id: RoomId,
    pub workspace_id: WorkspaceId,
    pub creator: UserId,
    pub creator_display_name: String,
    pub title: String,
    pub code: String,
    pub provider: BridgeProvider,
    pub meeting_code: String,
    pub join_url: Option<String>,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub now: DateTime<Utc>,
}

/// Outcome of `claim_bridge`.
#[derive(Debug, Clone)]
pub struct ClaimOutcome {
    pub room: RoomRecord,
    /// `true`: this call created the room.
    pub created: bool,
    /// `true`: the caller holds the capturer lease after the call.
    pub is_capturer: bool,
}

/// Capturer heartbeat report (proposal B8).
#[derive(Debug, Clone, Copy)]
pub struct Heartbeat {
    pub audio_flowing: bool,
    pub last_audio_at: Option<DateTime<Utc>>,
}

/// The acting user plus their workspace role, resolved by the service before the lock is taken.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub user: UserId,
    pub ws_role: Option<WorkspaceRole>,
}

impl Caller {
    pub fn is_ws_admin(&self) -> bool {
        matches!(
            self.ws_role,
            Some(WorkspaceRole::Owner | WorkspaceRole::Admin)
        )
    }
}

pub struct JoinInput {
    pub room: RoomId,
    pub user: UserId,
    pub display_name: Option<String>,
    pub speak_language: String,
    pub listen_language: String,
    pub now: DateTime<Utc>,
}

/// Result of a locked mutation; the service publishes `commands` and `snapshot` after commit.
#[derive(Debug, Clone)]
pub struct Mutation {
    pub room: RoomRecord,
    pub participant: Option<ParticipantRecord>,
    pub commands: Vec<RoomCommand>,
    /// Present when the route set or room state changed.
    pub snapshot: Option<AudioRoutesSnapshot>,
}

/// Mutating methods run in one transaction that locks the room row (`FOR UPDATE`).
#[async_trait]
pub trait RoomRepo: Send + Sync {
    async fn create(&self, new: NewRoom) -> AppResult<RoomRecord>;
    async fn list(
        &self,
        workspace: WorkspaceId,
        status: Option<RoomStatus>,
        page: &PageRequest,
    ) -> AppResult<Page<RoomRecord>>;
    async fn get(&self, id: RoomId) -> AppResult<Option<RoomRecord>>;
    async fn find_by_code(&self, code: &str) -> AppResult<Option<RoomRecord>>;
    async fn has_participant(&self, room: RoomId, user: UserId) -> AppResult<bool>;
    async fn participants(&self, room: RoomId) -> AppResult<Vec<ParticipantRecord>>;
    async fn routes_snapshot(
        &self,
        room: RoomId,
        now: DateTime<Utc>,
    ) -> AppResult<AudioRoutesSnapshot>;

    async fn lifecycle(
        &self,
        room: RoomId,
        caller: Caller,
        action: RoomAction,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation>;
    async fn join(&self, input: JoinInput) -> AppResult<Mutation>;
    async fn admit(
        &self,
        room: RoomId,
        participant: ParticipantId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation>;
    async fn kick(
        &self,
        room: RoomId,
        participant: ParticipantId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation>;
    async fn leave(&self, room: RoomId, user: UserId, now: DateTime<Utc>) -> AppResult<Mutation>;
    async fn generate_routes(
        &self,
        room: RoomId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation>;

    /// Find-or-create the open bridge room for `(workspace, provider, meeting_code)`; the caller
    /// becomes capturer when the room has none (or when it is new).
    async fn claim_bridge(&self, new: NewBridgeRoom, cfg: BridgeConfig) -> AppResult<ClaimOutcome>;
    /// Current capturer renews the lease. 409 `NOT_BRIDGE_ROOM` / `CAPTURER_LEASE_LOST`.
    async fn bridge_heartbeat(
        &self,
        room: RoomId,
        user: UserId,
        hb: Heartbeat,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord>;
    /// Reassign the capturer: allowed when stale, when none, by the current capturer or the host.
    async fn bridge_takeover(
        &self,
        room: RoomId,
        caller: Caller,
        cfg: BridgeConfig,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord>;
    /// Host or current capturer. Text to voice is refused while translation runs.
    async fn set_bridge_audio_mode(
        &self,
        room: RoomId,
        caller: Caller,
        mode: AudioMode,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord>;
}
