//! WarpBot: conversations, tool calls with confirmation gate, workspace RAG over Qdrant (schema assistant).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;
pub mod suggestion;
pub mod summary;
pub mod usage;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_web::JwtKeys;

use application::LlmConfigService;
use application::ports::{
    ConfigNotifier, KeyValidator, LlmConfigRepository, SecretVault, WorkspaceAccess,
};
use infrastructure::PgLlmConfigRepository;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "assistant",
    migrations: &[
        Migration {
            version: 1,
            name: "byo_llm",
            sql: include_str!("../migrations/0001_byo_llm.sql"),
        },
        Migration {
            version: 2,
            name: "meeting_summaries",
            sql: include_str!("../migrations/0002_meeting_summaries.sql"),
        },
    ],
};

pub struct AssistantDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub access: Arc<dyn WorkspaceAccess>,
    pub vault: Arc<dyn SecretVault>,
    pub validator: Arc<dyn KeyValidator>,
    pub notifier: Arc<dyn ConfigNotifier>,
}

/// Dependencies of the summary REST routes.
pub struct SummaryHttpDeps {
    pub service: Arc<summary::SummaryService>,
    pub access: Arc<dyn summary::RoomAccess>,
    pub clock: Arc<dyn Clock>,
}

pub struct AssistantModule {
    llm_config: Arc<LlmConfigService>,
    repo: Arc<dyn LlmConfigRepository>,
    jwt: JwtKeys,
    summary: Option<http::summary::SummaryApi>,
}

impl AssistantModule {
    pub fn new(deps: AssistantDeps) -> Self {
        let repo: Arc<dyn LlmConfigRepository> = Arc::new(PgLlmConfigRepository::new(deps.pool));
        let llm_config = Arc::new(LlmConfigService::new(
            repo.clone(),
            deps.vault,
            deps.validator,
            deps.notifier,
            deps.access,
            deps.clock,
        ));
        Self {
            llm_config,
            repo,
            jwt: deps.jwt,
            summary: None,
        }
    }

    /// Mount the post-session summary routes (`meetings/{room_id}/summary`, action items).
    pub fn with_summary(mut self, deps: SummaryHttpDeps) -> Self {
        self.summary = Some(http::summary::SummaryApi {
            service: deps.service,
            access: deps.access,
            clock: deps.clock,
        });
        self
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::AppState {
            llm_config: self.llm_config.clone(),
            jwt: self.jwt.clone(),
            summary: self.summary.clone(),
        })
    }

    /// Read access to workspace LLM configs and sealed credentials for the AI resolver.
    pub fn llm_config_repository(&self) -> Arc<dyn LlmConfigRepository> {
        self.repo.clone()
    }
}
