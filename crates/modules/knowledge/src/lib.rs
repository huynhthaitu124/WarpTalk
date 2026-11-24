//! Workspace knowledge & RAG: ingestion (chunking, PII gate, embeddings), vector search with
//! access scoping, re-rank, citations, facts.
//!
//! Layers: `domain` (pure), `application` (use cases + ports), `infrastructure` (Postgres,
//! Redis), `http`.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_llm::{EmbeddingProvider, LlmProvider};
use warptalk_qdrant::VectorStore;
use warptalk_settings::{NS_RAG, SettingsStore};
use warptalk_web::JwtKeys;

pub use application::{
    AccessContext, AccessScope, DocumentAccess, KnowledgeService, SearchFilters, SearchResult,
};
pub use domain::{RagConfig, SourceType};
pub use infrastructure::{
    IndexRequestHandler, PurgeRequestHandler, index_consumer_config, purge_consumer_config,
};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "knowledge",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct KnowledgeDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsStore,
    pub vectors: Arc<dyn VectorStore>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    /// Provider name recorded in the embedding profile (`openai`, ...).
    pub embedding_provider: String,
    /// Whether embeddings leave the platform (honours `external_llm_allowed`).
    pub embedding_is_external: bool,
    /// For LLM re-ranking and fact extraction; both degrade gracefully without it.
    pub llm: Option<Arc<dyn LlmProvider>>,
    pub access: Arc<dyn DocumentAccess>,
    pub results: Arc<dyn application::ResultPublisher>,
}

pub struct KnowledgeModule {
    state: http::HttpState,
    indexing: Arc<application::IndexingService>,
    repo: Arc<dyn application::KnowledgeRepository>,
}

impl KnowledgeModule {
    pub fn new(deps: KnowledgeDeps) -> Self {
        // Validate the extended `rag` namespace on `put` too.
        deps.settings.register::<RagConfig>(NS_RAG);
        let repo: Arc<dyn application::KnowledgeRepository> =
            Arc::new(infrastructure::PgKnowledgeRepository::new(deps.pool));
        let service = Arc::new(KnowledgeService::new(application::SearchDeps {
            repo: repo.clone(),
            vectors: deps.vectors.clone(),
            embeddings: deps.embeddings.clone(),
            settings: deps.settings.clone(),
            llm: deps.llm.clone(),
            metrics: Arc::new(application::MetricsRegistry::new()),
        }));
        let indexing = Arc::new(application::IndexingService::new(
            application::IndexingDeps {
                repo: repo.clone(),
                vectors: deps.vectors,
                embeddings: deps.embeddings,
                embedding_provider: deps.embedding_provider,
                embedding_is_external: deps.embedding_is_external,
                settings: deps.settings,
                results: deps.results,
                clock: deps.clock,
                llm: deps.llm,
            },
        ));
        Self {
            state: http::HttpState {
                service,
                access: deps.access,
                jwt: deps.jwt,
            },
            indexing,
            repo,
        }
    }

    pub fn router(&self) -> Router {
        http::router(self.state.clone())
    }

    /// Query API for other modules (assistant's `semantic_search`), wired by `apps/api`.
    pub fn service(&self) -> Arc<KnowledgeService> {
        self.state.service.clone()
    }

    pub fn indexing(&self) -> Arc<application::IndexingService> {
        self.indexing.clone()
    }

    /// Let the indexer read uploaded pdf/docx objects through `source` and extract their text
    /// (limits, OCR engine for scanned PDFs). Call right after [`KnowledgeModule::new`], before
    /// the handlers are taken.
    pub fn with_document_source(
        mut self,
        source: Arc<dyn application::DocumentSource>,
        limits: infrastructure::ExtractLimits,
        ocr: Arc<dyn application::OcrEngine>,
    ) -> Self {
        let chain = Arc::new(infrastructure::full_chain(limits, ocr));
        match Arc::get_mut(&mut self.indexing) {
            Some(svc) => {
                svc.set_extraction(source, chain);
            }
            None => tracing::error!("indexing service already shared; document source ignored"),
        }
        self
    }

    /// Handler for `embedding:index_requests` (run with [`index_consumer_config`]).
    pub fn index_handler(&self) -> Arc<dyn warptalk_bus::StreamHandler> {
        Arc::new(IndexRequestHandler::new(self.indexing.clone()))
    }

    /// Handler for `knowledge:purge_requests` (run with [`purge_consumer_config`]).
    pub fn purge_handler(&self) -> Arc<dyn warptalk_bus::StreamHandler> {
        Arc::new(PurgeRequestHandler::new(self.indexing.clone()))
    }

    /// Make a prepared profile the searched one (after a reindex into its collection).
    pub async fn activate_profile(
        &self,
        workspace: warptalk_kernel::WorkspaceId,
        profile_hash: &str,
    ) -> warptalk_kernel::AppResult<()> {
        self.repo
            .activate_profile(workspace.as_uuid(), profile_hash)
            .await
    }
}
