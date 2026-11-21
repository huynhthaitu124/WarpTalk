//! Live suggestion engine (spec 09): gated two-stage detector (decide, generate) over the live
//! transcript, structured follow-ups, grounded citations, BYO-provider ready.
//!
//! Everything tunable lives in [`config::SuggestionConfig`] (settings namespace `suggestion`,
//! resolved per room, hot-reloaded). Collaborators are ports in [`ports`]; Redis-backed adapters
//! for consent and room signals are in [`redis_ports`].

pub mod config;
pub mod config_source;
pub mod engine;
pub mod hygiene;
pub mod model;
pub mod ports;
pub mod prompt;
pub mod redis_ports;
pub mod slot;
pub mod stage0;
pub mod window;
pub mod worker;

pub use config::{SuggestionConfig, register as register_config};
pub use config_source::{ConfigSource, SettingsConfigSource, StaticConfig};
pub use engine::{EngineDeps, SuggestionEngine};
pub use model::{
    AI_ASSISTANT_RESULTS, ASSISTANT_FOLLOWUPS, FollowUp, FollowUpEvent, Outcome, SUGGESTION_GROUP,
    SuggestionResult,
};
pub use ports::{
    AllowAllConsent, ConsentPolicy, DataBoundary, GroundedSnippet, KnowledgeQuery,
    KnowledgeRetriever, LlmResolver, NoSignals, NoopRetriever, Purpose, ResolveError, ResolvedLlm,
    RoomDirectory, RoomInfo, RoomSignals,
};
pub use redis_ports::{RedisConsentPolicy, RedisRoomSignals};
pub use slot::SuggestionState;
pub use worker::SuggestionWorker;
