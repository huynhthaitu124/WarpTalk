//! PostgreSQL pool, error mapping, per-module migration runner, transactional outbox.

mod error;
mod migrate;
mod outbox;
mod pool;

pub use error::map_sqlx_error;
pub use migrate::{DbError, Migration, ModuleMigrations, run_migrations};
pub use outbox::{
    OUTBOX_MIGRATIONS, OutboxRow, claim_outbox_batch, enqueue_outbox, mark_failed, mark_published,
};
pub use pool::connect;
