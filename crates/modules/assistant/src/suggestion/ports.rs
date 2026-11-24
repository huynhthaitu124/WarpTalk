//! Ports the suggestion engine needs from the outside. `apps/api` / `apps/worker` wire adapters.

use std::sync::Arc;

use async_trait::async_trait;
use warptalk_kernel::{RoomId, WorkspaceId};
use warptalk_llm::LlmProvider;

/// What an LLM call is for; lets BYO resolution pick provider and model per purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    SuggestionDecide,
    SuggestionGenerate,
    /// Post-session summary / minutes / action items.
    Summary,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SuggestionDecide => "suggestion_decide",
            Self::SuggestionGenerate => "suggestion_generate",
            Self::Summary => "summary",
        }
    }
}

/// Where the provider processes data (byo-llm.md section 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataBoundary {
    /// Third-party endpoint: requires workspace consent (`external_llm_allowed`).
    External,
    /// Endpoint inside the workspace's own tenant: exempt from the consent gate.
    WorkspaceControlled,
}

#[derive(Clone)]
pub struct ResolvedLlm {
    pub provider: Arc<dyn LlmProvider>,
    /// Model chosen by the resolver (BYO). `None` means use the model from `SuggestionConfig`.
    pub model: Option<String>,
    pub boundary: DataBoundary,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("no provider configured for this workspace and purpose")]
    NotConfigured,
    #[error("provider resolution failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait LlmResolver: Send + Sync {
    async fn for_purpose(
        &self,
        workspace: WorkspaceId,
        purpose: Purpose,
    ) -> Result<ResolvedLlm, ResolveError>;
}

/// Fail-closed consent gate: may transcript text be sent to an external LLM?
#[async_trait]
pub trait ConsentPolicy: Send + Sync {
    /// Must return `false` when the answer is unknown or unreadable. `max_age` is how stale a
    /// cached answer may be (from `consent.cache_seconds`), so a revoke applies mid-meeting.
    async fn external_llm_allowed(
        &self,
        workspace: WorkspaceId,
        room: RoomId,
        max_age: std::time::Duration,
    ) -> bool;
}

/// Allows everything (tests, single-tenant deployments).
pub struct AllowAllConsent;

#[async_trait]
impl ConsentPolicy for AllowAllConsent {
    async fn external_llm_allowed(
        &self,
        _: WorkspaceId,
        _: RoomId,
        _: std::time::Duration,
    ) -> bool {
        true
    }
}

/// Room facts the gates need (gates 2 and 3).
#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub workspace_id: WorkspaceId,
    /// Not paused / ended / failed / cancelled / timed out.
    pub active: bool,
    /// Platform flag `flags.ai_suggest` for this room's workspace.
    pub ai_suggest_enabled: bool,
}

#[async_trait]
pub trait RoomDirectory: Send + Sync {
    /// `None` when the room is unknown (the engine then stays silent).
    async fn room_info(&self, room: RoomId) -> Option<RoomInfo>;
}

/// Per-segment room signals kept in Redis by other services.
#[async_trait]
pub trait RoomSignals: Send + Sync {
    /// Host-paused transcript segment (gate 4). Errors must read as `false`.
    async fn transcript_paused(&self, room: RoomId, segment_id: &str) -> bool;
    /// Listen languages of everyone in the room (any order, duplicates allowed).
    async fn reader_languages(&self, room: RoomId) -> Vec<String>;
}

/// No pause, no readers.
pub struct NoSignals;

#[async_trait]
impl RoomSignals for NoSignals {
    async fn transcript_paused(&self, _: RoomId, _: &str) -> bool {
        false
    }
    async fn reader_languages(&self, _: RoomId) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct KnowledgeQuery {
    pub workspace_id: WorkspaceId,
    pub room_id: RoomId,
    pub query: String,
    pub top_k: u32,
}

/// A grounded passage with enough identity to cite it.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundedSnippet {
    pub chunk_id: String,
    /// Document title shown on the citation chip.
    pub title: String,
    pub text: String,
    pub score: f32,
    /// Citation kind: `document` or `knowledge_fact`.
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RetrieveError {
    #[error("retrieval failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait KnowledgeRetriever: Send + Sync {
    async fn retrieve(&self, q: &KnowledgeQuery) -> Result<Vec<GroundedSnippet>, RetrieveError>;
}

/// Retrieves nothing; `fact` hints then decline ("fact needs docs").
pub struct NoopRetriever;

#[async_trait]
impl KnowledgeRetriever for NoopRetriever {
    async fn retrieve(&self, _: &KnowledgeQuery) -> Result<Vec<GroundedSnippet>, RetrieveError> {
        Ok(Vec::new())
    }
}
