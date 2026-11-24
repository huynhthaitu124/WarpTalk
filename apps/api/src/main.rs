//! Business plane: REST API, WebSocket hub, webhooks, scheduled jobs. Composition root that wires all modules.

use std::net::SocketAddr;

use anyhow::{Context, bail};
use warptalk_config::{
    DatabaseConfig, HttpConfig, JwtConfig, LogFormat as ConfigLogFormat, RedisConfig,
    TelemetryConfig,
};
use warptalk_telemetry::LogFormat as TelemetryLogFormat;
use warptalk_web::JwtKeys;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = TelemetryConfig::from_env().context("loading telemetry config")?;
    let db = DatabaseConfig::from_env().context("loading database config")?;
    let jwt_cfg = JwtConfig::from_env().context("loading jwt config")?;
    let http = HttpConfig::from_env().context("loading http config")?;
    let redis_cfg = RedisConfig::from_env().context("loading redis config")?;

    let format = match telemetry.log_format {
        ConfigLogFormat::Pretty => TelemetryLogFormat::Pretty,
        ConfigLogFormat::Json => TelemetryLogFormat::Json,
    };
    warptalk_telemetry::init_tracing("api", format, &telemetry.log_filter)
        .context("initialising tracing")?;
    let metrics_bind = telemetry
        .metrics_bind
        .as_deref()
        .map(str::parse::<SocketAddr>)
        .transpose()
        .context("parsing METRICS_BIND")?;
    warptalk_telemetry::init_metrics(metrics_bind).context("initialising metrics")?;

    let pool = warptalk_db::connect(&db.url, db.max_connections)
        .await
        .context("connecting to PostgreSQL")?;

    for module in ["identity", "assistant", "copilot"] {
        let migrated: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM public.schema_migrations WHERE module = $1)",
        )
        .bind(module)
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
        if !migrated {
            bail!(
                "database is not migrated (no '{module}' migrations found): run warptalk-migrator first"
            );
        }
    }

    let redis = warptalk_bus::connect(&redis_cfg.url)
        .await
        .context("connecting to Redis")?;

    let livekit = warptalk_api::LiveKitSettings::from_env().map(|l| l.into_config());
    if livekit.is_none() {
        tracing::warn!("LIVEKIT_* not set: meeting join answers 503 LIVEKIT_NOT_CONFIGURED");
    }
    let opts = warptalk_api::ApiOptions {
        livekit,
        room_service: None,
        redis_url: Some(redis_cfg.url.clone()),
        ai: None,
        vectors: None,
        vault: None,
        key_validator: None,
        invitation_mailer: None,
        object_store: None,
        documents_config: None,
        cartesia_api_key: std::env::var("CARTESIA_API_KEY").ok(),
        voice_cloner: None,
        voice_admin: None,
        stt_token_issuer: None,
    };
    let (verification_mailer, invitation_mailer) =
        warptalk_api::auth_wiring::mailers(&redis_cfg.url).context("configuring email delivery")?;
    let opts = warptalk_api::ApiOptions {
        invitation_mailer: Some(invitation_mailer),
        ..opts
    };

    let jwt = JwtKeys::from_config(&jwt_cfg);
    let app = warptalk_api::build_router(
        pool,
        jwt,
        &jwt_cfg,
        &http.cors_origins,
        verification_mailer,
        Some(redis),
        opts,
    )
    .await;

    let listener = tokio::net::TcpListener::bind(&http.bind)
        .await
        .with_context(|| format!("binding {}", http.bind))?;
    tracing::info!(bind = %http.bind, "api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving http")?;
    tracing::info!("api stopped");
    Ok(())
}

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
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
