//! Ports of the WarpBot chat use cases. `apps/api` wires the adapters (LLM resolution, consent,
//! knowledge search); storage and the event bus live in `infrastructure`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppResult, UserId, WorkspaceId};

use crate::domain::chat::{ChatMessage, Conversation, MessageStatus, ToolCallRecord};
use crate::suggestion::{ResolveError, ResolvedLlm};

/// Storage of conversations, messages and tool calls. Every query is scoped by `workspace_id`.
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn create(&self, c: &Conversation) -> AppResult<()>;
    /// The user's non-archived conversations, most recent activity first.
    async fn list(&self, ws: WorkspaceId, user: UserId) -> AppResult<Vec<Conversation>>;
    /// The conversation if `user` owns it. Detail routes carry no workspace id, so this lookup is
    /// owner-scoped; the caller then checks membership of the returned `workspace_id`.
    async fn find_owned(&self, user: UserId, id: Uuid) -> AppResult<Option<Conversation>>;
    async fn archive(&self, ws: WorkspaceId, user: UserId, id: Uuid) -> AppResult<bool>;
    async fn set_title(&self, ws: WorkspaceId, id: Uuid, title: &str) -> AppResult<()>;
    async fn touch(&self, ws: WorkspaceId, id: Uuid, at: DateTime<Utc>) -> AppResult<()>;

    async fn insert_message(&self, m: &ChatMessage) -> AppResult<()>;
    async fn list_messages(
        &self,
        ws: WorkspaceId,
        conversation: Uuid,
    ) -> AppResult<Vec<ChatMessage>>;
    async fn get_message(&self, ws: WorkspaceId, id: Uuid) -> AppResult<Option<ChatMessage>>;
    /// Final content of a reply.
    async fn finish_message(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        status: MessageStatus,
        content: &str,
        sources_json: Option<&str>,
        at: DateTime<Utc>,
    ) -> AppResult<()>;

    async fn insert_tool_call(&self, t: &ToolCallRecord) -> AppResult<()>;
    async fn finish_tool_call(
        &self,
        id: Uuid,
        ok: bool,
        result_json: &str,
        at: DateTime<Utc>,
    ) -> AppResult<()>;
}

/// One event on a conversation's live channel. `name` is the hub event name the client listens
/// for (`AssistantMessageChunk`, ...), `data` its payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatEvent {
    pub name: String,
    pub data: Value,
}

/// Live channel per conversation plus the stop flag of a reply. Implementations must allow a
/// reader on another replica to resume after an event id (`Last-Event-ID`).
#[async_trait]
pub trait ChatEventBus: Send + Sync {
    async fn publish(&self, conversation: Uuid, event: ChatEvent) -> AppResult<()>;
    /// Id to read after so that only events published from now on are returned.
    async fn cursor_now(&self, conversation: Uuid) -> AppResult<String>;
    /// Events after `cursor` (oldest first) with their ids. Returns immediately.
    async fn read_after(
        &self,
        conversation: Uuid,
        cursor: &str,
    ) -> AppResult<Vec<(String, ChatEvent)>>;
    async fn request_stop(&self, message: Uuid) -> AppResult<()>;
    async fn stop_requested(&self, message: Uuid) -> AppResult<bool>;
}

/// The workspace's assistant model (`LlmPurpose::Assistant`).
#[async_trait]
pub trait AssistantLlmResolver: Send + Sync {
    async fn resolve(&self, ws: WorkspaceId) -> Result<ResolvedLlm, ResolveError>;
}

/// Workspace consent to send content to a third-party model (`externalLlmAllowed`).
#[async_trait]
pub trait WorkspaceLlmConsent: Send + Sync {
    async fn external_llm_allowed(&self, ws: WorkspaceId) -> AppResult<bool>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeHit {
    pub title: String,
    pub text: String,
    /// document | transcript | meeting | glossary | knowledge
    pub kind: String,
    pub ref_id: Option<String>,
    pub score: f32,
}

/// Semantic search over what the user may read in the workspace (documents, meetings, glossary).
#[async_trait]
pub trait WorkspaceKnowledge: Send + Sync {
    async fn search(
        &self,
        ws: WorkspaceId,
        user: UserId,
        query: &str,
        limit: usize,
    ) -> AppResult<Vec<KnowledgeHit>>;
}
