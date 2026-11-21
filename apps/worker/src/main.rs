use anyhow::{Context, bail};
use tokio::sync::watch;
use warptalk_config::{DatabaseConfig, LogFormat as ConfigLogFormat, RedisConfig, TelemetryConfig};
use warptalk_telemetry::LogFormat as TelemetryLogFormat;
use warptalk_worker::ai::AiProviders;
use warptalk_worker::knowledge::build_vector_store;
use warptalk_worker::{
    build_ai_plane, build_settings, build_transcript_module, join_consumers, knowledge_consumers,
    spawn_consumers, spawn_suggestion,
};

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

    let redis = warptalk_bus::connect(&redis_cfg.url)
        .await
        .context("connecting to Redis")?;
    let settings = build_settings(pool.clone(), redis, &redis_cfg.url);
    let ai = AiProviders::from_env(settings.clone());
    let plane = build_ai_plane(
        pool.clone(),
        &redis_cfg.url,
        build_vector_store()?,
        ai,
        settings,
    )
    .await?;

    let (tx, rx) = watch::channel(false);
    let mut consumers = transcript.consumers();
    consumers.extend(knowledge_consumers(&plane.knowledge));
    let mut handles = spawn_consumers(&redis_cfg.url, consumers, &rx).await?;
    handles.push(spawn_suggestion(&redis_cfg.url, plane.suggestion.clone(), &rx).await?);
    tracing::info!(consumers = handles.len(), "worker started");

    shutdown_signal().await;
    tracing::info!("shutdown signal received");
    let _ = tx.send(true);
    join_consumers(handles).await;
    pool.close().await;
    tracing::info!("worker stopped");
    Ok(())
}
