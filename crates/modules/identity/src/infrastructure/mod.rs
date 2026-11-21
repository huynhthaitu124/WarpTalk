//! Adapters: repositories (sqlx), mailer adapters.

mod dev_mail;
mod log_mailer;
mod pg_repo;

pub use dev_mail::DevMailCapture;
pub use log_mailer::LogMailer;
pub use pg_repo::PgIdentityRepository;
