//! Ports: what this module needs from the outside (workspace policy, voice consent, event
//! publishing) plus the persistence port implemented in `infrastructure`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use warptalk_contracts::events::{AudioRoutesSnapshot, RoomCommand, RouteStatus};
use warptalk_kernel::{
    AppError, AppResult, Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId,
    WorkspaceRole,
};

use crate::domain::{
    AudioMode, BridgeConfig, BridgeProvider, BridgeState, ParticipantStatus, RoomAction,
    RoomSettings, RoomSettingsPatch, RoomStatus, RoomType,
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

// ---------------------------------------------------------------- invitations port

/// Emails newly invited to a room (outbound notification, e.g. mail).
#[derive(Debug, Clone)]
pub struct RoomInvite {
    pub room_id: RoomId,
    pub workspace_id: WorkspaceId,
    pub room_title: String,
    pub room_code: String,
    pub inviter: UserId,
    pub emails: Vec<String>,
}

#[async_trait]
pub trait RoomInviteNotifier: Send + Sync {
    async fn notify(&self, invite: RoomInvite) -> AppResult<()>;
}

/// Default: logs the room and the number of invitees (never the addresses).
pub struct LogInviteNotifier;

#[async_trait]
impl RoomInviteNotifier for LogInviteNotifier {
    async fn notify(&self, invite: RoomInvite) -> AppResult<()> {
        tracing::info!(
            room_id = %invite.room_id,
            invitees = invite.emails.len(),
            "room invitations created"
        );
        Ok(())
    }
}

/// Test double that records every notification.
#[derive(Default)]
pub struct RecordingInviteNotifier {
    pub sent: Mutex<Vec<RoomInvite>>,
}

#[async_trait]
impl RoomInviteNotifier for RecordingInviteNotifier {
    async fn notify(&self, invite: RoomInvite) -> AppResult<()> {
        self.sent
            .lock()
            .map_err(|_| AppError::internal("invite notifier poisoned"))?
            .push(invite);
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
    pub description: Option<String>,
    pub code: String,
    pub status: RoomStatus,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub settings: RoomSettings,
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
    pub description: Option<String>,
    pub code: String,
    pub status: RoomStatus,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub settings: RoomSettings,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
}

/// A copilot session room (type `COPILOT_SESSION`): created `IN_PROGRESS` with the host as the
/// only (connected) participant.
pub struct NewCopilotRoom {
    pub id: RoomId,
    pub workspace_id: WorkspaceId,
    pub host: UserId,
    pub host_display_name: String,
    pub title: String,
    pub code: String,
    /// Language of the host's own channel (stored as the room's source language).
    pub source_language: String,
    /// Language of the other side (stored as the single target language).
    pub target_language: String,
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
    /// Raw display name of the caller (roster name when they join as listener).
    pub display_name: Option<String>,
    /// Caller's caption language; `None`: keep the existing one, else the first target language.
    pub listen_language: Option<String>,
    /// Options applied only when this claim creates the room.
    pub settings: RoomSettings,
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
    /// Route snapshot to publish when the caller joining as listener changed a live room's routes.
    pub snapshot: Option<AudioRoutesSnapshot>,
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

pub struct UpdateLanguagesInput {
    pub room: RoomId,
    pub user: UserId,
    /// `None`: keep. `none` (listen-only) is allowed.
    pub speak_language: Option<String>,
    pub listen_language: Option<String>,
    pub now: DateTime<Utc>,
}

/// Host edit of room properties. `None`: keep; `Some(None)`: clear (nullable fields).
pub struct UpdateRoomInput {
    pub room: RoomId,
    pub caller: Caller,
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub scheduled_at: Option<Option<DateTime<Utc>>>,
    pub source_language: Option<String>,
    pub target_languages: Option<Vec<String>>,
    pub settings: RoomSettingsPatch,
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
    /// Live `EXTERNAL_BRIDGE` rooms whose capturer last heartbeated before `idle_cutoff`, or whose
    /// capturer never heartbeated at all (lease still equals the claim time) and was claimed before
    /// `unclaimed_cutoff`, with the user allowed to end each one (effective host).
    async fn idle_bridge_rooms(
        &self,
        idle_cutoff: DateTime<Utc>,
        unclaimed_cutoff: DateTime<Utc>,
    ) -> AppResult<Vec<(RoomId, UserId)>> {
        let _ = (idle_cutoff, unclaimed_cutoff);
        Ok(Vec::new())
    }
    /// Pending invitation emails of a room, oldest first.
    async fn invitations(&self, room: RoomId) -> AppResult<Vec<String>>;
    /// Make the room's PENDING invitations exactly `emails` (`replace`) or add `emails` to them;
    /// returns the emails newly invited by this call.
    async fn set_invitations(
        &self,
        room: RoomId,
        emails: &[String],
        replace: bool,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<String>>;
    /// Insert an `IN_PROGRESS` copilot room and its host participant; returns the room and the
    /// host's participant id. `Conflict DUPLICATE` when the room code is taken.
    async fn create_copilot(&self, new: NewCopilotRoom) -> AppResult<(RoomRecord, ParticipantId)>;
    async fn list(
        &self,
        workspace: WorkspaceId,
        status: Option<RoomStatus>,
        page: &PageRequest,
    ) -> AppResult<Page<RoomRecord>>;
    async fn get(&self, id: RoomId) -> AppResult<Option<RoomRecord>>;
    /// Recurring-meeting series of a room (`None` when standalone or unknown).
    async fn series_of(&self, id: RoomId) -> AppResult<Option<uuid::Uuid>>;
    /// Rooms of `workspace` the user hosts or took part in, plus the display names the user
    /// used in the workspace's rooms (distinct).
    async fn user_scope(
        &self,
        workspace: WorkspaceId,
        user: UserId,
    ) -> AppResult<(Vec<RoomId>, Vec<String>)>;
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
    /// Set the runtime status of a current route (`CAPTION_ONLY`, `STANDARD_VOICE`, ...). `false`
    /// when the route is not current in that room or already `COMPLETED`.
    async fn set_route_status(
        &self,
        room: RoomId,
        route: uuid::Uuid,
        status: RouteStatus,
    ) -> AppResult<bool>;
    async fn leave(&self, room: RoomId, user: UserId, now: DateTime<Utc>) -> AppResult<Mutation>;
    /// Change the caller's own speak/listen language; reconciles routes when the room is live.
    async fn update_languages(&self, input: UpdateLanguagesInput) -> AppResult<Mutation>;
    /// Host edits title/description/schedule/languages/lobby. Participants whose languages left
    /// the room fall back to the source language; routes reconcile when live.
    async fn update_room(&self, input: UpdateRoomInput) -> AppResult<Mutation>;
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
    /// Current capturer or host clears the capturer immediately (idempotent when there is none).
    async fn bridge_release(
        &self,
        room: RoomId,
        caller: Caller,
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
