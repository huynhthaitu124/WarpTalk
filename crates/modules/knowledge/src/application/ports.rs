//! Ports this module needs from the outside (wired by `apps/api` / `apps/worker`).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};

use crate::domain::{ChunkKind, EmbeddingProfile, FactCategory, IndexResult, SourceType};

/// What the caller may retrieve, resolved by the workspace/meeting modules.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessScope {
    pub role: WorkspaceRole,
    /// Documents the caller may have the assistant answer from (spec 14 section 4.2).
    pub allowed_document_ids: Vec<Uuid>,
    /// Meetings the caller may open (transcripts, summaries).
    pub allowed_meeting_ids: Vec<Uuid>,
}

/// Allowlist provider. `Err(Forbidden)` when `user` is not a member of `workspace`; failures must
/// surface as errors (the caller then returns nothing: fail closed).
#[async_trait]
pub trait DocumentAccess: Send + Sync {
    async fn access_scope(&self, workspace: WorkspaceId, user: UserId) -> AppResult<AccessScope>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileStatus {
    Active,
    Building,
    Retired,
}

impl ProfileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Building => "building",
            Self::Retired => "retired",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "building" => Self::Building,
            _ => Self::Retired,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRow {
    pub workspace_id: Uuid,
    pub profile: EmbeddingProfile,
    pub collection: String,
    pub status: ProfileStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Retrying,
    Indexed,
    Blocked,
    Deleted,
    Dead,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Indexed => "indexed",
            Self::Blocked => "blocked",
            Self::Deleted => "deleted",
            Self::Dead => "dead",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "retrying" => Self::Retrying,
            "indexed" => Self::Indexed,
            "blocked" => Self::Blocked,
            "deleted" => Self::Deleted,
            _ => Self::Dead,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub job_id: String,
    pub workspace_id: Uuid,
    pub source_type: String,
    pub source_id: String,
    pub status: JobStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub chunks_indexed: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobStart {
    /// Process now; `attempts` counts this attempt.
    Started { attempts: u32 },
    /// A redelivery arrived before its backoff elapsed: do nothing, leave the message pending.
    Backoff { until: DateTime<Utc> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRecord {
    pub id: Uuid,
    pub ordinal: u32,
    pub kind: ChunkKind,
    pub text_hash: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FactRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_type: SourceType,
    pub source_id: String,
    pub category: FactCategory,
    pub fact: String,
    pub quote: Option<String>,
    pub confidence: f32,
    pub source_chunk_ids: Vec<Uuid>,
    /// The fact's own indexed chunk (kind = fact).
    pub chunk_id: Option<Uuid>,
}

/// A mirrored chunk row (what the management API lists and edits).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredChunk {
    pub id: Uuid,
    pub source_type: String,
    pub source_id: String,
    pub kind: ChunkKind,
    pub ordinal: u32,
    pub collection: String,
    pub payload: Value,
    pub indexed_at: DateTime<Utc>,
}

/// Keyset position of a chunk listing (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkCursor {
    pub indexed_at: DateTime<Utc>,
    pub id: Uuid,
}

/// Filter of a chunk listing. Transcript chunks are never listed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkListFilter {
    /// `None` = every listable source type.
    pub source_types: Option<Vec<String>>,
    pub fact_category: Option<FactCategory>,
    /// `Some` limits documents to these ids (members); `None` = unrestricted (owner/admin).
    pub allowed_document_ids: Option<Vec<Uuid>>,
    /// `Some` limits meeting summaries to these meetings.
    pub allowed_meeting_ids: Option<Vec<Uuid>>,
}

/// One source (document, transcript, ...) within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKey<'a> {
    pub workspace: Uuid,
    pub source_type: SourceType,
    pub source_id: &'a str,
}

#[async_trait]
pub trait KnowledgeRepository: Send + Sync {
    // --- embedding profiles ---
    async fn active_profile(&self, workspace: Uuid) -> AppResult<Option<ProfileRow>>;
    /// Record `desired` for the workspace: reuse its row, else create it (`active` when the
    /// workspace has no active profile yet, otherwise `building`).
    async fn register_profile(
        &self,
        workspace: Uuid,
        desired: &EmbeddingProfile,
    ) -> AppResult<ProfileRow>;
    /// Make `profile_hash` the one searched; the previous active becomes `retired`.
    async fn activate_profile(&self, workspace: Uuid, profile_hash: &str) -> AppResult<()>;
    /// All profiles, including retired ones (their collections still hold vectors).
    async fn profiles(&self, workspace: Uuid) -> AppResult<Vec<ProfileRow>>;

    // --- index jobs ---
    async fn begin_job(
        &self,
        workspace: Uuid,
        job_id: &str,
        source_type: SourceType,
        source_id: &str,
        message_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<JobStart>;
    async fn fail_attempt(
        &self,
        workspace: Uuid,
        job_id: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn finish_job(
        &self,
        workspace: Uuid,
        job_id: &str,
        status: JobStatus,
        chunks_indexed: u32,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn job(&self, workspace: Uuid, job_id: &str) -> AppResult<Option<JobRecord>>;

    // --- chunk mirror ---
    /// Upsert `records` and delete rows of this (source, kind) that are not among them.
    async fn replace_chunks(
        &self,
        src: SourceKey<'_>,
        kind: ChunkKind,
        collection: &str,
        records: &[ChunkRecord],
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn count_chunks(&self, src: SourceKey<'_>) -> AppResult<u64>;

    // --- facts ---
    async fn replace_facts(
        &self,
        src: SourceKey<'_>,
        facts: &[FactRecord],
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn list_facts(
        &self,
        workspace: Uuid,
        source_type: Option<SourceType>,
        category: Option<FactCategory>,
        limit: u32,
    ) -> AppResult<Vec<FactRecord>>;

    // --- chunk management ---
    /// Newest first by `(indexed_at, id)`, strictly after `after`.
    async fn list_chunks(
        &self,
        workspace: Uuid,
        filter: &ChunkListFilter,
        after: Option<ChunkCursor>,
        limit: u32,
    ) -> AppResult<Vec<StoredChunk>>;
    /// A manageable (non-transcript) chunk of the workspace.
    async fn get_chunk(&self, workspace: Uuid, id: Uuid) -> AppResult<Option<StoredChunk>>;
    /// Replace the payload of a fact chunk and sync its `knowledge.facts` row.
    async fn update_fact_chunk(
        &self,
        workspace: Uuid,
        id: Uuid,
        category: FactCategory,
        fact: &str,
        payload: &Value,
        text_hash: &str,
    ) -> AppResult<()>;
    /// Delete the chunk row and the fact row owned by it. Returns whether a chunk row existed.
    async fn delete_chunk(&self, workspace: Uuid, id: Uuid) -> AppResult<bool>;

    /// Delete chunk rows and facts of a source (both filters `None` = the whole workspace).
    /// Returns `(chunks, facts)` deleted.
    async fn purge_rows(
        &self,
        workspace: Uuid,
        source_type: Option<SourceType>,
        source_id: Option<&str>,
    ) -> AppResult<(u64, u64)>;
}

/// Publishes `embedding:index_results`.
#[async_trait]
pub trait ResultPublisher: Send + Sync {
    async fn publish(&self, result: &IndexResult) -> Result<(), String>;
}

/// Reads the raw bytes of an uploaded document (pdf/docx) for text extraction. Wired by
/// `apps/worker` over the object store.
#[async_trait]
pub trait DocumentSource: Send + Sync {
    async fn fetch(&self, object_key: &str) -> Result<Vec<u8>, DocumentSourceError>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DocumentSourceError {
    /// The object does not exist (retrying cannot help).
    #[error("object not found: {0}")]
    NotFound(String),
    /// Storage hiccup (retry).
    #[error("{0}")]
    Backend(String),
}
