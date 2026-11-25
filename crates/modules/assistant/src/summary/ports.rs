//! Ports of the summary feature. `apps/worker` and `apps/api` wire the adapters.

use async_trait::async_trait;
use uuid::Uuid;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId, WorkspaceRole};

use super::model::SummaryRequest;

/// One final transcript segment.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub id: Uuid,
    pub speaker: String,
    pub text: String,
    pub language: String,
    pub start_ms: i64,
}

#[derive(Debug, Clone)]
pub struct TranscriptData {
    pub workspace_id: WorkspaceId,
    /// Version of the room's current transcript (idempotency key of the automatic job).
    pub version: i32,
    /// True once the transcript was finalized.
    pub finalized: bool,
    /// Final segments in order, at most `limit` of them.
    pub segments: Vec<TranscriptSegment>,
    /// Segments stored in total.
    pub total_segments: u32,
}

/// Read-only view of the transcript tables.
#[async_trait]
pub trait TranscriptSource: Send + Sync {
    /// `None` when the room has no transcript. `limit` caps the segments returned (oldest first).
    async fn load(&self, room: RoomId, limit: u32) -> AppResult<Option<TranscriptData>>;

    /// Like [`Self::load`] but keeps the NEWEST `limit` segments (still returned oldest first).
    /// Live Q&A cares about the end of a long meeting; the default delegates to `load`.
    async fn load_recent(&self, room: RoomId, limit: u32) -> AppResult<Option<TranscriptData>> {
        self.load(room, limit).await
    }
}

#[derive(Debug, Clone)]
pub struct RoomMeta {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub participants: Vec<String>,
}

#[async_trait]
pub trait RoomMetaSource: Send + Sync {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomMeta>>;

    /// Who is who in the room (host user, copilot speaker names, series). Optional: sources
    /// that cannot tell return `None` and the summary falls back to the raw speaker names.
    async fn context(&self, _room: RoomId) -> AppResult<Option<RoomContext>> {
        Ok(None)
    }
}

/// The copilot speaker names of a room (`copilot.sessions.name_me` / `name_others`): the
/// transcript stores exactly these as speaker names of the two channels.
#[derive(Debug, Clone, PartialEq)]
pub struct CopilotNames {
    /// Channel `me`: the host's microphone (default "Tôi").
    pub me: String,
    /// Channel `others`: everybody else, not told apart (default "Người khác").
    pub others: String,
}

/// Directory facts about a room that the summary and the meeting memory need.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomContext {
    pub host_user_id: Option<UserId>,
    /// The host user's real display name (`identity.users.full_name`).
    pub host_name: Option<String>,
    /// `Some` for copilot rooms (type `COPILOT_SESSION`).
    pub copilot: Option<CopilotNames>,
    /// Recurring-meeting series of the room, when it belongs to one.
    pub series_id: Option<Uuid>,
    pub title: String,
    /// Participants with a user account: (display name in the room, user id).
    pub participant_users: Vec<(String, UserId)>,
}

/// A follow-up the live suggestion engine already emitted for the room.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFollowUp {
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

#[async_trait]
pub trait FollowUpSource: Send + Sync {
    /// Oldest first, at most `limit` (the most recent ones when more exist).
    async fn for_room(&self, room: RoomId, limit: u32) -> Vec<LiveFollowUp>;
}

/// No live follow-ups.
pub struct NoFollowUps;

#[async_trait]
impl FollowUpSource for NoFollowUps {
    async fn for_room(&self, _: RoomId, _: u32) -> Vec<LiveFollowUp> {
        Vec::new()
    }
}

/// Announces work to the worker (`assistant:summary_requests`) and results to listeners.
#[async_trait]
pub trait SummaryBus: Send + Sync {
    async fn request(&self, req: &SummaryRequest) -> Result<(), String>;
    async fn result(&self, ev: &super::model::SummaryResultEvent);
    /// Dead letter for a job that exhausted its attempts.
    async fn dead_letter(&self, req: &SummaryRequest, error: &str);
}

/// Publishes nothing (tests, API without Redis).
pub struct NoBus;

#[async_trait]
impl SummaryBus for NoBus {
    async fn request(&self, _: &SummaryRequest) -> Result<(), String> {
        Ok(())
    }
    async fn result(&self, _: &super::model::SummaryResultEvent) {}
    async fn dead_letter(&self, _: &SummaryRequest, _: &str) {}
}

/// What a user is to a room.
#[derive(Debug, Clone)]
pub struct RoomAccessInfo {
    pub workspace_id: WorkspaceId,
    pub is_host: bool,
    pub is_participant: bool,
    /// The user's role in the room's workspace (`None` = not a member).
    pub workspace_role: Option<WorkspaceRole>,
}

impl RoomAccessInfo {
    pub fn is_admin(&self) -> bool {
        matches!(
            self.workspace_role,
            Some(WorkspaceRole::Owner | WorkspaceRole::Admin)
        )
    }

    /// Host, participant or workspace OWNER/ADMIN.
    pub fn can_view(&self) -> bool {
        self.is_host || self.is_participant || self.is_admin()
    }

    /// Host or workspace OWNER/ADMIN.
    pub fn can_manage(&self) -> bool {
        self.is_host || self.is_admin()
    }
}

/// A user's standing in a workspace, for the cross-room action item list.
#[derive(Debug, Clone)]
pub struct WorkspaceScope {
    pub role: WorkspaceRole,
    /// Rooms of the workspace the user hosts or takes part in.
    pub room_ids: Vec<RoomId>,
    /// Names the user goes by (display name...), to match `owner` for `mine=true`.
    pub names: Vec<String>,
}

#[async_trait]
pub trait RoomAccess: Send + Sync {
    /// `None` when the room does not exist.
    async fn room_access(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomAccessInfo>>;
    /// `None` when the user is not a member of the workspace.
    async fn workspace_scope(
        &self,
        workspace: WorkspaceId,
        user: UserId,
    ) -> AppResult<Option<WorkspaceScope>>;
}

/// Durable workspace consent for sending transcript text to a third-party LLM
/// (`workspace.workspaces.settings.externalLlmAllowed`). Unlike the live Redis per-room key it
/// is still there after the meeting ended. Fail-closed: `false` whenever the answer is unknown.
#[async_trait]
pub trait ExternalLlmGate: Send + Sync {
    async fn external_llm_allowed(&self, workspace: WorkspaceId) -> bool;
}

/// Allows everything (tests, single-tenant deployments).
pub struct AllowAllExternalLlm;

#[async_trait]
impl ExternalLlmGate for AllowAllExternalLlm {
    async fn external_llm_allowed(&self, _: WorkspaceId) -> bool {
        true
    }
}
