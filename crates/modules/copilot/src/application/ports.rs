//! Ports of the copilot module. `apps/api` wires the adapters (room, workspace) and the
//! Redis/OpenAI implementations live in `infrastructure`.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_contracts::events::SttResult;
use warptalk_kernel::{AppError, AppResult, RoomId, UserId, WorkspaceId, WorkspaceRole};

use crate::domain::Platform;

// ---------------------------------------------------------------- persistence

/// Stored session (`copilot.sessions`).
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub room_id: RoomId,
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    pub host_participant_id: Uuid,
    pub others_speaker_id: Uuid,
    pub title: String,
    pub platform: Platform,
    pub meeting_url: Option<String>,
    pub language_me: String,
    pub language_others: String,
    pub name_me: String,
    pub name_others: String,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait SessionRepo: Send + Sync {
    async fn insert(&self, s: &SessionRecord) -> AppResult<()>;
    async fn get(&self, room: RoomId) -> AppResult<Option<SessionRecord>>;
    /// Newest first, plus the total count. `host`: only sessions hosted by that user; `None`: the
    /// whole workspace.
    async fn list(
        &self,
        ws: WorkspaceId,
        host: Option<UserId>,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<SessionRecord>, i64)>;
}

// ---------------------------------------------------------------- room / workspace

/// A room created for a session.
#[derive(Debug, Clone)]
pub struct CreatedRoom {
    pub room_id: RoomId,
    pub host_participant_id: Uuid,
    pub started_at: DateTime<Utc>,
}

pub struct NewSessionRoom {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub host_display_name: String,
    pub host_language: String,
    pub other_language: String,
}

/// Room state the session needs (`status` is the room status string, e.g. `IN_PROGRESS`).
#[derive(Debug, Clone)]
pub struct RoomState {
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl RoomState {
    pub fn in_progress(&self) -> bool {
        self.status == "IN_PROGRESS"
    }
}

/// The room module, seen from copilot (api wires `RoomQueries`). Creating a session room must also
/// leave the room usable by transcripts, suggestions and follow-ups (ai policy keys).
#[async_trait]
pub trait SessionRooms: Send + Sync {
    /// Create an `IN_PROGRESS` room of type `COPILOT_SESSION` hosted by `user`.
    async fn create(&self, user: UserId, new: NewSessionRoom) -> AppResult<CreatedRoom>;
    async fn state(&self, room: RoomId) -> AppResult<Option<RoomState>>;
    /// End the room as `actor` (host or workspace owner/admin); idempotent. Publishes the
    /// room-ended side effects (`__MEETING_END__` on `stt:results`).
    async fn end(&self, room: RoomId, actor: UserId) -> AppResult<()>;
}

#[async_trait]
pub trait WorkspaceMembers: Send + Sync {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
}

// ---------------------------------------------------------------- stt:results

/// Outcome of [`SegmentSink::publish`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    Written,
    /// The same `(room, clientSegmentId)` was already accepted.
    Duplicate,
}

/// Writes segments to `stt:results` (+ the per-meeting copy), once per dedup key.
#[async_trait]
pub trait SegmentSink: Send + Sync {
    async fn publish(&self, result: &SttResult, dedup_key: &str) -> AppResult<Published>;
}

// ---------------------------------------------------------------- stt token

/// What the app needs to open a streaming transcription connection straight to the provider.
#[derive(Clone)]
pub struct SttToken {
    pub provider: String,
    pub model: String,
    /// Short-lived secret; never logged.
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub websocket_url: String,
    /// Session configuration the client sends when it connects (provider specific JSON).
    pub session_config: Value,
}

impl std::fmt::Debug for SttToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttToken")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

pub struct SttTokenRequest {
    pub session: RoomId,
    /// Primary language hint (the channel language of the host), if known.
    pub language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SttIssueError {
    #[error("speech-to-text is not configured")]
    NotConfigured,
    #[error("speech-to-text provider unavailable: {0}")]
    Upstream(String),
}

#[async_trait]
pub trait SttTokenIssuer: Send + Sync {
    async fn issue(&self, req: SttTokenRequest) -> Result<SttToken, SttIssueError>;
}

/// Issuer used when no platform key is configured.
pub struct NoSttTokenIssuer;

#[async_trait]
impl SttTokenIssuer for NoSttTokenIssuer {
    async fn issue(&self, _: SttTokenRequest) -> Result<SttToken, SttIssueError> {
        Err(SttIssueError::NotConfigured)
    }
}

/// Fixed-window counter per key.
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// `true` while the caller is within `limit` hits per `window_secs`.
    async fn allow(&self, key: &str, limit: u32, window_secs: u64) -> AppResult<bool>;
}

/// No limit (tests, Redis-less runs).
pub struct NoRateLimit;

#[async_trait]
impl RateLimiter for NoRateLimit {
    async fn allow(&self, _: &str, _: u32, _: u64) -> AppResult<bool> {
        Ok(true)
    }
}

// ---------------------------------------------------------------- in-memory fakes (tests)

/// In-memory sink: records results, dedups by key.
#[derive(Default)]
pub struct MemorySegmentSink {
    pub seen: Mutex<HashMap<String, ()>>,
    pub results: Mutex<Vec<SttResult>>,
}

#[async_trait]
impl SegmentSink for MemorySegmentSink {
    async fn publish(&self, result: &SttResult, dedup_key: &str) -> AppResult<Published> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| AppError::internal("poisoned"))?;
        if seen.insert(dedup_key.to_owned(), ()).is_some() {
            return Ok(Published::Duplicate);
        }
        self.results
            .lock()
            .map_err(|_| AppError::internal("poisoned"))?
            .push(result.clone());
        Ok(Published::Written)
    }
}
