//! Knowledge wiring shared by the worker and the API: vector store selection, the suggestion
//! engine's retriever over `KnowledgeService`, and module construction.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_assistant::suggestion::ports::RetrieveError;
use warptalk_assistant::suggestion::{GroundedSnippet, KnowledgeQuery, KnowledgeRetriever};
use warptalk_kernel::{AppError, AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::application::{
    DocumentSource, DocumentSourceError, NoopOcr, ResultPublisher,
};
use warptalk_knowledge::domain::CitationKind;
use warptalk_knowledge::infrastructure::ExtractLimits;
use warptalk_knowledge::{
    AccessContext, AccessScope, DocumentAccess, KnowledgeDeps, KnowledgeModule, KnowledgeService,
    SearchFilters,
};
use warptalk_qdrant::{InMemoryVectorStore, QdrantClient, VectorStore};
use warptalk_settings::SettingsStore;
use warptalk_storage::{ObjectStore, StorageError};
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
    /// Object reader for pdf/docx text extraction (indexing consumers only).
    pub documents: Option<Arc<dyn DocumentSource>>,
}

/// `DocumentSource` over the workspace documents' object store.
pub struct ObjectStoreDocumentSource(pub Arc<dyn ObjectStore>);

#[async_trait]
impl DocumentSource for ObjectStoreDocumentSource {
    async fn fetch(&self, object_key: &str) -> Result<Vec<u8>, DocumentSourceError> {
        match self.0.get(object_key).await {
            Ok(b) => Ok(b.to_vec()),
            Err(StorageError::NotFound(m) | StorageError::InvalidKey(m)) => {
                Err(DocumentSourceError::NotFound(m))
            }
            Err(e) => Err(DocumentSourceError::Backend(e.to_string())),
        }
    }
}

/// The object store selected by `STORAGE_PROVIDER` (same as the API's), as a document source.
/// `None` (logged) when storage is not configured: pdf/docx then stay `skipped`.
pub fn document_source_from_env() -> Option<Arc<dyn DocumentSource>> {
    match warptalk_storage::StorageConfig::from_env()
        .and_then(|cfg| warptalk_storage::build_store(&cfg))
    {
        Ok(store) => Some(Arc::new(ObjectStoreDocumentSource(store))),
        Err(e) => {
            tracing::error!(error = %e, "object storage unavailable: pdf/docx documents will not be indexed");
            None
        }
    }
}

fn env_num<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// Extraction limits: defaults 50 MB / 500 pages / 120 s, overridable with
/// `KNOWLEDGE_EXTRACT_MAX_MB`, `KNOWLEDGE_EXTRACT_MAX_PAGES`, `KNOWLEDGE_EXTRACT_TIMEOUT_SECS`.
pub fn extract_limits_from_env() -> ExtractLimits {
    let mut l = ExtractLimits::default();
    if let Some(mb) = env_num::<usize>("KNOWLEDGE_EXTRACT_MAX_MB") {
        l.max_bytes = mb.saturating_mul(1024 * 1024);
    }
    if let Some(n) = env_num("KNOWLEDGE_EXTRACT_MAX_PAGES") {
        l.max_pdf_pages = n;
    }
    if let Some(s) = env_num("KNOWLEDGE_EXTRACT_TIMEOUT_SECS") {
        l.timeout = Duration::from_secs(s);
    }
    l
}

pub async fn build_knowledge_module(p: KnowledgeParts) -> KnowledgeModule {
    if !p.ai.any_key() {
        tracing::warn!(
            "no platform AI keys (OPENAI_API_KEY / GEMINI_API_KEY): knowledge embeddings disabled, indexing and search will fail"
        );
    }
    let documents = p.documents;
    let module = KnowledgeModule::new(KnowledgeDeps {
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
    });
    match documents {
        // TODO(ocr): pass a real engine here to index scanned PDFs / images.
        Some(src) => module.with_document_source(src, extract_limits_from_env(), Arc::new(NoopOcr)),
        None => module,
    }
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
