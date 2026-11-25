//! WarpBot "ask the meeting": grounded Q&A over the room's transcript and workspace documents
//! (SSE or plain JSON), quick recap, per-user history, usage metering and rate limits.
//!
//! Everything tunable lives in [`config::ChatConfig`] (settings namespace `assistant_chat`).
//! Collaborators are ports: knowledge retrieval and the consent policy are the suggestion
//! engine's, transcript / room facts / room access are the summary feature's, earlier-meeting
//! context is [`ports::PriorContextSource`].

pub mod config;
pub mod http;
pub mod model;
pub mod ports;
pub mod prompt;
pub mod repo;
pub mod service;

pub use config::{ChatConfig, NS_CHAT, register as register_config};
pub use http::ChatApi;
pub use model::{
    AskRequest, ChatAnswer, ChatEvent, ChatScope, Citation, HistoryItem, RecapRequest,
};
pub use ports::{
    CachedRecap, MemoryRateLimiter, MemoryRecapCache, NoopPriorContext, PriorContextItem,
    PriorContextSource, RateLimiter, RecapCache, RedisRateLimiter, RedisRecapCache,
    SummaryMemoryPriorContext,
};
pub use repo::PgChatRepository;
pub use service::{ChatDeps, ChatService, FEATURE, Prepared};
