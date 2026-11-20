//! Adapters: repositories (sqlx), mailer adapters.

mod log_mailer;
mod pg_repo;

pub use log_mailer::LogMailer;
pub use pg_repo::PgIdentityRepository;
