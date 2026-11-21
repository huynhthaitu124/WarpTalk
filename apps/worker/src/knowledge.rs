//! Knowledge wiring shared by the worker and the API: vector store selection, the suggestion
//! engine's retriever over `KnowledgeService`, and module construction.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_assistant::suggestion::{
    GroundedSnippet, KnowledgeQuery, KnowledgeRetriever, ports::RetrieveError,
};
use warptalk_kernel::{AppError, AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::application::ports::ResultPublisher;
use warptalk_knowledge::domain::CitationKind;
use warptalk_knowledge::{
    AccessContext, AccessScope, DocumentAccess, KnowledgeDeps, KnowledgeModule, KnowledgeService,
    SearchFilters,
};
use warptalk_qdrant::{InMemoryVectorStore, QdrantClient, VectorStore};
use warptalk_settings::SettingsStore;
use warptalk_web::JwtKeys;

use crate::ai::AiProviders;

/// `QdrantClient` when `QDRANT_URL` is set (optional `QDRANT_API_KEY`), else an in-memory store
/// (nothing persists; fine for local runs only).
pub fn build_vector_store() -> anyhow::Result<Arc<dyn VectorStore>> {
    match std::env::var("QDRANT_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        Some(url) => {
            let key = std::env::var("QDRANT_API_KEY").ok();
            let client = QdrantClient::new(url, key, Duration::from_secs(30))
                .context("building Qdrant client")?;
            Ok(Arc::new(client))
        }
        None => {
            tracing::warn!("QDRANT_URL not set: using an in-memory vector store (not persistent)");
            Ok(Arc::new(InMemoryVectorStore::new()))
        }
    }
}

/// Refuses everything: for processes that mount no knowledge HTTP routes.
pub struct DenyDocumentAccess;

#[async_trait]
impl DocumentAccess for DenyDocumentAccess {
    async fn access_scope(&self, _: WorkspaceId, _: UserId) -> AppResult<AccessScope> {
        Err(AppError::forbidden("knowledge routes are not served here"))
    }
}

pub struct KnowledgeParts {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub settings: SettingsStore,
    pub vectors: Arc<dyn VectorStore>,
    pub ai: Arc<AiProviders>,
    pub access: Arc<dyn DocumentAccess>,
    pub results: Arc<dyn ResultPublisher>,
}

pub async fn build_knowledge_module(p: KnowledgeParts) -> KnowledgeModule {
    if !p.ai.any_key() {
        tracing::warn!(
            "no platform AI keys (OPENAI_API_KEY / GEMINI_API_KEY): knowledge embeddings disabled, indexing and search will fail"
        );
    }
    KnowledgeModule::new(KnowledgeDeps {
        pool: p.pool,
        jwt: p.jwt,
        clock: Arc::new(SystemClock),
        settings: p.settings,
        vectors: p.vectors,
        embeddings: p.ai.embedder(),
        embedding_provider: p.ai.embedding_provider_name().await,
        // Platform keys send text to third-party endpoints.
        embedding_is_external: true,
        llm: p.ai.chat(),
        access: p.access,
        results: p.results,
    })
}

/// Suggestion retriever: a system-level search limited to the room's workspace. The search
/// itself only returns chunks whose `ai_retrieval` flag is set and which are active.
pub struct ServiceRetriever {
    service: Arc<KnowledgeService>,
}

impl ServiceRetriever {
    pub fn new(service: Arc<KnowledgeService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl KnowledgeRetriever for ServiceRetriever {
    async fn retrieve(&self, q: &KnowledgeQuery) -> Result<Vec<GroundedSnippet>, RetrieveError> {
        // Nil user id marks a system caller; Admin role = unscoped within this one workspace.
        let ctx = AccessContext::privileged(
            q.workspace_id,
            UserId::from_uuid(Uuid::nil()),
            WorkspaceRole::Admin,
        );
        let filters = SearchFilters {
            limit: Some(q.top_k as usize),
            ..SearchFilters::default()
        };
        let res = self
            .service
            .search(&ctx, &q.query, &filters)
            .await
            .map_err(|e| RetrieveError::Failed(e.to_string()))?;
        Ok(res
            .chunks
            .into_iter()
            .map(|h| GroundedSnippet {
                chunk_id: h.chunk_id.to_string(),
                title: h.citation.title.clone(),
                text: h.payload.text.clone(),
                score: h.score,
                kind: match h.citation.kind {
                    CitationKind::Knowledge => "knowledge_fact",
                    _ => "document",
                }
                .to_owned(),
            })
            .collect())
    }
}
