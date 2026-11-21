use anyhow::{Context, bail};
use tokio::sync::watch;
use warptalk_config::{DatabaseConfig, LogFormat as ConfigLogFormat, RedisConfig, TelemetryConfig};
use warptalk_telemetry::LogFormat as TelemetryLogFormat;
use warptalk_worker::{build_transcript_module, join_consumers, spawn_consumers};

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = TelemetryConfig::from_env().context("loading telemetry config")?;
    let db = DatabaseConfig::from_env().context("loading database config")?;
    let redis_cfg = RedisConfig::from_env().context("loading redis config")?;

    let format = match telemetry.log_format {
        ConfigLogFormat::Pretty => TelemetryLogFormat::Pretty,
        ConfigLogFormat::Json => TelemetryLogFormat::Json,
    };
    warptalk_telemetry::init_tracing("worker", format, &telemetry.log_filter)
        .context("initialising tracing")?;

    let pool = warptalk_db::connect(&db.url, db.max_connections)
        .await
        .context("connecting to PostgreSQL")?;
    let migrated: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.schema_migrations WHERE module = 'transcript')",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);
    if !migrated {
        bail!(
            "database is not migrated (no 'transcript' migrations found): run warptalk-migrator first"
        );
    }

    let transcript = build_transcript_module(pool.clone(), &redis_cfg.url).await?;

    let (tx, rx) = watch::channel(false);
    let handles = spawn_consumers(&redis_cfg.url, transcript.consumers(), &rx).await?;
    tracing::info!(consumers = handles.len(), "worker started");

    shutdown_signal().await;
    tracing::info!("shutdown signal received");
    let _ = tx.send(true);
    join_consumers(handles).await;
    pool.close().await;
    tracing::info!("worker stopped");
    Ok(())
}
