//! Adapters: repositories (sqlx), Redis publishers/consumers, provider clients.

mod chat_bus;
mod pg_chat_repo;
mod pg_llm_repo;

pub use chat_bus::{InMemoryChatEventBus, RedisChatEventBus};
pub use pg_chat_repo::PgConversationRepository;
pub use pg_llm_repo::PgLlmConfigRepository;
