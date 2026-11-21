//! Adapters: sqlx persistence, Redis chat translation consumer.

pub mod chat_translate;
pub mod pg_repo;

pub use chat_translate::{
    CHAT_TRANSLATE_GROUP, ChatTranslateHandler, RedisChatEventSink, chat_channel,
    chat_translate_consumer_config,
};
pub use pg_repo::PgMeetingRepo;
