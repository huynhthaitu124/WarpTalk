//! Adapters: Postgres repository, Redis result publisher, stream handlers.

mod consumers;
mod pg_repo;
mod publisher;

pub use consumers::{
    INDEX_GROUP, INDEX_RESULTS_STREAM, IndexRequestHandler, KNOWLEDGE_PURGE_REQUESTS, PURGE_GROUP,
    PurgeRequestHandler, index_consumer_config, purge_consumer_config,
};
pub use pg_repo::PgKnowledgeRepository;
pub use publisher::{MemoryResultPublisher, RedisResultPublisher};
