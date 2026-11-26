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

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_web::JwtKeys;

use application::chat_ports::{
    AssistantLlmResolver, ChatEventBus, WorkspaceKnowledge, WorkspaceLlmConsent,
};
use application::ports::{
    ConfigNotifier, KeyValidator, LlmConfigRepository, SecretVault, WorkspaceAccess,
};
use application::{ChatService, ChatServiceDeps, LlmConfigService};
use infrastructure::{PgConversationRepository, PgLlmConfigRepository};

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
            name: "conversations",
            sql: include_str!("../migrations/0002_conversations.sql"),
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

/// What WarpBot chat needs beyond [`AssistantDeps`]. Wired after the AI plane and knowledge
/// modules exist (they depend on this module), see [`AssistantModule::enable_chat`].
pub struct ChatDeps {
    pub bus: Arc<dyn ChatEventBus>,
    pub llm: Arc<dyn AssistantLlmResolver>,
    pub consent: Arc<dyn WorkspaceLlmConsent>,
    /// `None` when there is no knowledge index: the search tool is not offered.
    pub knowledge: Option<Arc<dyn WorkspaceKnowledge>>,
    /// Model used when the resolver does not pick one.
    pub default_model: Option<String>,
}

pub struct AssistantModule {
    llm_config: Arc<LlmConfigService>,
    repo: Arc<dyn LlmConfigRepository>,
    jwt: JwtKeys,
    pool: PgPool,
    access: Arc<dyn WorkspaceAccess>,
    clock: Arc<dyn Clock>,
    chat: Option<ChatService>,
}

impl AssistantModule {
    pub fn new(deps: AssistantDeps) -> Self {
        let repo: Arc<dyn LlmConfigRepository> =
            Arc::new(PgLlmConfigRepository::new(deps.pool.clone()));
        let llm_config = Arc::new(LlmConfigService::new(
            repo.clone(),
            deps.vault,
            deps.validator,
            deps.notifier,
            deps.access.clone(),
            deps.clock.clone(),
        ));
        Self {
            llm_config,
            repo,
            jwt: deps.jwt,
            pool: deps.pool,
            access: deps.access,
            clock: deps.clock,
            chat: None,
        }
    }

    /// Turns on WarpBot chat (`/assistant/conversations...`). Without it those routes are absent.
    pub fn enable_chat(&mut self, deps: ChatDeps) {
        self.chat = Some(ChatService::new(ChatServiceDeps {
            repo: Arc::new(PgConversationRepository::new(self.pool.clone())),
            bus: deps.bus,
            llm: deps.llm,
            consent: deps.consent,
            knowledge: deps.knowledge,
            access: self.access.clone(),
            clock: self.clock.clone(),
            default_model: deps.default_model,
        }));
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        let router = http::router(http::AppState {
            llm_config: self.llm_config.clone(),
            jwt: self.jwt.clone(),
        });
        match &self.chat {
            Some(chat) => router.merge(http::chat::router(http::chat::ChatState {
                chat: chat.clone(),
                jwt: self.jwt.clone(),
            })),
            None => router,
        }
    }

    /// Read access to workspace LLM configs and sealed credentials for the AI resolver.
    pub fn llm_config_repository(&self) -> Arc<dyn LlmConfigRepository> {
        self.repo.clone()
    }
}
