//! Applies forward-only SQL migrations of every module, then exits. Must succeed before api/worker start.

mod registry;

use anyhow::Context;
use warptalk_config::{DatabaseConfig, LogFormat as ConfigLogFormat, TelemetryConfig};
use warptalk_telemetry::LogFormat as TelemetryLogFormat;

use crate::registry::registry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = TelemetryConfig::from_env().context("loading telemetry config")?;
    let format = match telemetry.log_format {
        ConfigLogFormat::Pretty => TelemetryLogFormat::Pretty,
        ConfigLogFormat::Json => TelemetryLogFormat::Json,
    };
    warptalk_telemetry::init_tracing("migrator", format, &telemetry.log_filter)
        .context("initialising tracing")?;

    let db = DatabaseConfig::from_env().context("loading database config")?;
    let pool = warptalk_db::connect(&db.url, 2)
        .await
        .context("connecting to PostgreSQL")?;

    let sets = registry();
    warptalk_db::run_migrations(&pool, &sets)
        .await
        .context("applying migrations")?;

    for set in &sets {
        tracing::info!(
            module = set.module,
            migrations = set.migrations.len(),
            "module migrations up to date"
        );
    }
    tracing::info!("all migrations applied");
    pool.close().await;
    Ok(())
}
