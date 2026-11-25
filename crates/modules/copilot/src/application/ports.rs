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

use crate::domain::{Channel, LanguageMode, Platform};

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
    /// How to hint the provider (`fixed` keeps the single `language`).
    pub language_mode: LanguageMode,
    /// Other languages to expect besides `language` (base codes).
    pub languages: Vec<String>,
    /// Vocabulary prompt (glossary terms), for providers that take one.
    pub prompt: Option<String>,
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

// ---------------------------------------------------------------- usage metering + credit gate

/// Redis stream of billable usage (`usage:events`, consumed by the billing metering worker).
pub const USAGE_STREAM: &str = "usage:events";

/// One entry of `usage:events` (JSON in the stream field `data`). `event_id` is the idempotency
/// key: the same segment always yields the same id, so redelivery never double-charges. Wire
/// contract shared with `warptalk-billing` (`domain::UsageEvent`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageEvent {
    pub event_id: String,
    pub workspace_id: Uuid,
    pub room_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    /// `stt` here.
    pub feature: &'static str,
    pub model: String,
    pub audio_ms: Option<i64>,
    /// `platform` (the app streams with the platform's ephemeral token).
    pub boundary: &'static str,
    pub occurred_at: DateTime<Utc>,
    pub segment_id: Option<Uuid>,
}

#[async_trait]
pub trait UsageSink: Send + Sync {
    async fn emit(&self, event: &UsageEvent) -> AppResult<()>;
}

/// Drops events (metering not wired).
pub struct NoUsageSink;

#[async_trait]
impl UsageSink for NoUsageSink {
    async fn emit(&self, _: &UsageEvent) -> AppResult<()> {
        Ok(())
    }
}

/// Is the workspace allowed to use paid AI right now (billing's AI service state)? `api` wires
/// the billing adapter. Implementations fail OPEN: an unreadable state never blocks a meeting.
#[async_trait]
pub trait CreditGate: Send + Sync {
    /// `Some(reason)` (e.g. `insufficient_credits`) when AI usage is suspended.
    async fn suspended(&self, ws: WorkspaceId) -> Option<String>;
}

pub struct NoCreditGate;

#[async_trait]
impl CreditGate for NoCreditGate {
    async fn suspended(&self, _: WorkspaceId) -> Option<String> {
        None
    }
}

/// Metering hooks of the service.
#[derive(Clone)]
pub struct Metering {
    pub usage: std::sync::Arc<dyn UsageSink>,
    pub gate: std::sync::Arc<dyn CreditGate>,
    /// STT model name recorded on usage (the one the token issuer configures).
    pub stt_model: String,
}

/// Records events in memory (tests).
#[derive(Default)]
pub struct MemoryUsageSink {
    pub events: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl UsageSink for MemoryUsageSink {
    async fn emit(&self, event: &UsageEvent) -> AppResult<()> {
        let v = serde_json::to_value(event).map_err(|e| AppError::internal(e.to_string()))?;
        self.events
            .lock()
            .map_err(|_| AppError::internal("poisoned"))?
            .push(v);
        Ok(())
    }
}

/// Gate that answers a fixed reason (tests).
pub struct FixedCreditGate(pub Mutex<Option<String>>);

#[async_trait]
impl CreditGate for FixedCreditGate {
    async fn suspended(&self, _: WorkspaceId) -> Option<String> {
        self.0.lock().ok().and_then(|g| g.clone())
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

// ---------------------------------------------------------------- server-side STT (fallback)

/// Per-channel provider session parameters.
#[derive(Debug, Clone)]
pub struct ServerSttParams {
    pub session: RoomId,
    pub channel: Channel,
    pub mode: LanguageMode,
    /// Primary language hint (`None` in `multi`).
    pub language: Option<String>,
    /// All languages to expect, primary first.
    pub languages: Vec<String>,
    /// Vocabulary prompt.
    pub prompt: Option<String>,
}

/// Output of a server STT session, in commit order per channel: zero or more `Segment`s, then
/// exactly one `UtteranceDone`, for each commit.
#[derive(Debug, Clone)]
pub enum ServerSttEvent {
    Segment {
        text: String,
        /// Resolved language (adapters apply the vi/en heuristic); `None` = unknown.
        language: Option<String>,
        confidence: Option<f32>,
        /// More of the same utterance follows.
        early: bool,
    },
    UtteranceDone { ok: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum ServerSttError {
    #[error("server speech-to-text is not configured")]
    NotConfigured,
    #[error("speech-to-text provider error: {0}")]
    Provider(String),
}

/// Streaming STT run by the server for the fallback audio socket. `apps/api` wires an adapter
/// over `warptalk-stt` (modules do not depend on pipeline crates); [`FakeServerStt`] serves
/// tests and key-less demos.
///
/// [`FakeServerStt`]: crate::infrastructure::fake_stt::FakeServerStt
#[async_trait]
pub trait ServerStt: Send + Sync {
    async fn open(
        &self,
        params: ServerSttParams,
        events: tokio::sync::mpsc::Sender<ServerSttEvent>,
    ) -> Result<Box<dyn ServerSttSession>, ServerSttError>;
    /// Model name recorded on usage events.
    fn model(&self) -> String;
}

#[async_trait]
pub trait ServerSttSession: Send {
    /// Speech audio of the utterance in progress (PCM16, 16 kHz, mono), in order.
    async fn feed(&mut self, pcm: &[i16]) -> Result<(), ServerSttError>;
    /// The utterance ended: transcribe it. Returns once the commit is issued.
    async fn commit(&mut self) -> Result<(), ServerSttError>;
    /// Throw away the audio fed since the last commit.
    async fn discard(&mut self) -> Result<(), ServerSttError>;
    /// Finish in-flight commits, then release.
    async fn close(self: Box<Self>);
}

/// Claims behind a one-time audio-socket ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTicketClaims {
    pub user: UserId,
    pub session: RoomId,
}

/// Short-lived, single-use tickets for browsers / WKWebView, which cannot set an
/// `Authorization` header on a WebSocket. The key is a hash of the ticket (never the ticket).
#[async_trait]
pub trait AudioTicketStore: Send + Sync {
    async fn put(&self, key: &str, claims: AudioTicketClaims, expires_at: DateTime<Utc>)
    -> AppResult<()>;
    /// Remove and return the claims; `None` when unknown, used or expired at `now`.
    async fn take(&self, key: &str, now: DateTime<Utc>) -> AppResult<Option<AudioTicketClaims>>;
}

/// In-process store (tests, single instance).
#[derive(Default)]
pub struct MemoryTicketStore {
    items: Mutex<HashMap<String, (AudioTicketClaims, DateTime<Utc>)>>,
}

#[async_trait]
impl AudioTicketStore for MemoryTicketStore {
    async fn put(
        &self,
        key: &str,
        claims: AudioTicketClaims,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        self.items
            .lock()
            .map_err(|_| AppError::internal("poisoned"))?
            .insert(key.to_owned(), (claims, expires_at));
        Ok(())
    }

    async fn take(&self, key: &str, now: DateTime<Utc>) -> AppResult<Option<AudioTicketClaims>> {
        let mut m = self
            .items
            .lock()
            .map_err(|_| AppError::internal("poisoned"))?;
        m.retain(|_, (_, exp)| *exp > now);
        Ok(m.remove(key).map(|(c, _)| c))
    }
}

/// Glossary terms of a workspace, as vocabulary hints. Best effort: failures yield no terms.
#[async_trait]
pub trait GlossaryTerms: Send + Sync {
    async fn terms(&self, ws: WorkspaceId, user: UserId, max: usize) -> Vec<String>;
}

pub struct NoGlossary;

#[async_trait]
impl GlossaryTerms for NoGlossary {
    async fn terms(&self, _: WorkspaceId, _: UserId, _: usize) -> Vec<String> {
        Vec::new()
    }
}
