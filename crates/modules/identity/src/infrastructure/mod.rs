//! Adapters: repositories (sqlx), mailer adapters, voice provider/runtime adapters.

mod cartesia_voice;
mod dev_mail;
mod log_mailer;
mod pg_repo;
mod pg_voice_repo;
mod redis_voice_state;

pub use cartesia_voice::CartesiaVoice;
pub use dev_mail::DevMailCapture;
pub use log_mailer::LogMailer;
pub use pg_repo::PgIdentityRepository;
pub use pg_voice_repo::PgVoiceRepository;
pub use redis_voice_state::{RedisVoiceStatePublisher, consent_key};
