//! Ports: what the transcript module needs from the outside, plus its persistence and Redis ports.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppResult, RoomId, TranscriptId, UserId, WorkspaceId};

use crate::domain::{Correction, Segment, SegmentView, Transcript};

// ---------------------------------------------------------------- external policy / directory ports

/// Room facts needed to create a transcript head (api/worker wire the room module's data).
#[derive(Debug, Clone)]
pub struct RoomDetails {
    pub workspace_id: WorkspaceId,
}

#[async_trait]
pub trait RoomDirectory: Send + Sync {
    /// `None` when the translation room does not exist.
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomDetails>>;
}

/// Spec 12 §5.4 (WT-587): rooms with SaveTranscript=false are never persisted.
#[async_trait]
pub trait RoomPolicy: Send + Sync {
    async fn save_transcript(&self, room: RoomId) -> AppResult<bool>;
}

/// Workspace AI usage policy (spec 12 §6.2, spec 14).
#[async_trait]
pub trait WorkspaceAiPolicy: Send + Sync {
    async fn ai_retrieval_allowed(&self, workspace: WorkspaceId) -> AppResult<bool>;
    async fn external_llm_allowed(&self, workspace: WorkspaceId) -> AppResult<bool>;
}

/// Effective host of a room (follows host transfer).
#[async_trait]
pub trait RoomHost: Send + Sync {
    /// `None` when the room does not exist.
    async fn effective_host(&self, room: RoomId) -> AppResult<Option<UserId>>;
}

/// What a read-access decision is about.
#[derive(Debug, Clone, Copy)]
pub struct AccessTarget {
    pub workspace_id: WorkspaceId,
    pub room_id: RoomId,
}

/// Spec 12 §5.1: participant of the room, or workspace OWNER/ADMIN. (The effective
/// host is admitted by the service itself.)
#[async_trait]
pub trait TranscriptReadAccess: Send + Sync {
    async fn can_read(&self, user: UserId, target: AccessTarget) -> AppResult<bool>;
}

// ---------------------------------------------------------------- redis-backed ports

/// Pause gate (WT-605): while paused, segments are not stored and their ids are remembered.
#[async_trait]
pub trait PauseGate: Send + Sync {
    async fn is_paused(&self, room: RoomId) -> AppResult<bool>;
    async fn mark_skipped(&self, room: RoomId, segment: Uuid) -> AppResult<()>;
    async fn was_skipped(&self, room: RoomId, segment: Uuid) -> AppResult<bool>;
}

/// Entry on `embedding:index_requests` (spec 12 §6.2).
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingIndexRequest {
    pub job_id: Uuid,
    pub workspace_id: WorkspaceId,
    pub collection_id: String,
    pub source_type: String,
    pub source_id: Uuid,
    pub chunks_json: String,
    pub external_llm_allowed: bool,
    pub ai_retrieval_allowed: bool,
    pub retention_state: String,
    pub deletion_state: String,
    pub timestamp_ms: i64,
}

#[async_trait]
pub trait EmbeddingPublisher: Send + Sync {
    async fn publish(&self, req: &EmbeddingIndexRequest) -> AppResult<()>;
}

// ---------------------------------------------------------------- persistence port

#[derive(Debug, Clone)]
pub struct NewHead {
    pub id: TranscriptId,
    pub workspace_id: WorkspaceId,
    pub room_id: RoomId,
    pub source_language: String,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewSegment {
    pub id: Uuid,
    pub transcript_id: TranscriptId,
    pub speaker_participant_id: Option<Uuid>,
    pub speaker_name: String,
    pub original_text: String,
    pub original_language: String,
    pub start_time_ms: Option<i32>,
    pub end_time_ms: Option<i32>,
    pub confidence: Option<f64>,
    pub is_final: bool,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewTranslation {
    pub segment_id: Uuid,
    pub workspace_id: WorkspaceId,
    pub target_language: String,
    /// Sentence index inside the segment. `None`: the text translates the whole segment and
    /// replaces any chunks.
    pub chunk_index: Option<u32>,
    /// This sentence's text (the segment's whole text when `chunk_index` is `None`).
    pub text: String,
    /// Language the pieces are joined in (`warptalk_contracts::text::join_sentences`).
    pub joined_with_lang: String,
    pub translator_model: String,
    pub latency_ms: Option<i32>,
    pub is_retranslated: bool,
    pub previous_content_id: Option<Uuid>,
    pub source_stt_confidence: Option<f64>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewDubbing {
    pub segment_id: Uuid,
    pub target_language: String,
    pub voice_type: String,
    pub provider: String,
    pub provider_voice_id: String,
    pub duration_ms: Option<i32>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewCorrection {
    pub transcript_id: TranscriptId,
    pub segment_id: Uuid,
    pub user_id: UserId,
    pub text: String,
    pub reason: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CorrectionOutcome {
    Created(Correction),
    /// Transcript is FINALIZED or ARCHIVED.
    Locked,
    /// Segment is not part of the transcript.
    SegmentNotFound,
}

#[async_trait]
pub trait TranscriptRepo: Send + Sync {
    async fn current_for_room(&self, room: RoomId) -> AppResult<Option<Transcript>>;
    async fn by_id(&self, id: TranscriptId) -> AppResult<Option<Transcript>>;
    /// Return the room's current head, creating it (linked to the previous one) if absent.
    async fn ensure_head(&self, head: &NewHead) -> AppResult<Transcript>;
    /// Insert idempotently on `id`, advancing the transcript counters. False if it already existed.
    async fn insert_segment(&self, seg: &NewSegment) -> AppResult<bool>;
    async fn segment_with_transcript(&self, id: Uuid) -> AppResult<Option<(Segment, Transcript)>>;
    /// Store the sentence, then upsert the concatenation of the segment's sentences so far
    /// (dedup on workspace/hash/lang) and make it the segment's current link.
    async fn record_translation(&self, t: &NewTranslation) -> AppResult<()>;
    /// Upsert dubbing metadata. False when the segment has no current link for the language yet.
    async fn record_dubbing(&self, d: &NewDubbing) -> AppResult<bool>;
    async fn list_segments(
        &self,
        transcript: TranscriptId,
        lang: Option<&str>,
    ) -> AppResult<Vec<SegmentView>>;
    async fn add_correction(&self, c: &NewCorrection) -> AppResult<CorrectionOutcome>;
    async fn list_corrections(
        &self,
        transcript: TranscriptId,
        segment: Option<Uuid>,
    ) -> AppResult<Vec<Correction>>;
    /// Set FINALIZED (is_active=false, finalized_at) from RECORDING/FINALIZING.
    async fn finalize(&self, id: TranscriptId, now: DateTime<Utc>) -> AppResult<()>;
}

// ---------------------------------------------------------------- noop / in-memory impls (tests, wiring)

/// Persists every room.
pub struct AlwaysSaveRoomPolicy;

#[async_trait]
impl RoomPolicy for AlwaysSaveRoomPolicy {
    async fn save_transcript(&self, _: RoomId) -> AppResult<bool> {
        Ok(true)
    }
}

/// Fixed AI policy for every workspace.
pub struct StaticAiPolicy {
    pub retrieval: bool,
    pub external_llm: bool,
}

#[async_trait]
impl WorkspaceAiPolicy for StaticAiPolicy {
    async fn ai_retrieval_allowed(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(self.retrieval)
    }
    async fn external_llm_allowed(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(self.external_llm)
    }
}

/// Denies every read.
pub struct NoopReadAccess;

#[async_trait]
impl TranscriptReadAccess for NoopReadAccess {
    async fn can_read(&self, _: UserId, _: AccessTarget) -> AppResult<bool> {
        Ok(false)
    }
}

/// Settable in-memory read access keyed by (user, room).
#[derive(Default)]
pub struct InMemoryReadAccess {
    allowed: Mutex<Vec<(UserId, RoomId)>>,
}

impl InMemoryReadAccess {
    pub fn allow(&self, user: UserId, room: RoomId) {
        self.allowed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((user, room));
    }
}

#[async_trait]
impl TranscriptReadAccess for InMemoryReadAccess {
    async fn can_read(&self, user: UserId, target: AccessTarget) -> AppResult<bool> {
        Ok(self
            .allowed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(user, target.room_id)))
    }
}

/// Knows no hosts.
pub struct NoopRoomHost;

#[async_trait]
impl RoomHost for NoopRoomHost {
    async fn effective_host(&self, _: RoomId) -> AppResult<Option<UserId>> {
        Ok(None)
    }
}

/// Settable in-memory room directory and host (tests).
#[derive(Default)]
pub struct InMemoryRooms {
    rooms: Mutex<HashMap<RoomId, (WorkspaceId, UserId)>>,
}

impl InMemoryRooms {
    pub fn set(&self, room: RoomId, workspace: WorkspaceId, host: UserId) {
        self.rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(room, (workspace, host));
    }
}

#[async_trait]
impl RoomDirectory for InMemoryRooms {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomDetails>> {
        Ok(self
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&room)
            .map(|(w, _)| RoomDetails { workspace_id: *w }))
    }
}

#[async_trait]
impl RoomHost for InMemoryRooms {
    async fn effective_host(&self, room: RoomId) -> AppResult<Option<UserId>> {
        Ok(self
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&room)
            .map(|(_, h)| *h))
    }
}
