use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

/// Connect to PostgreSQL with a 5s acquire timeout.
pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
}
