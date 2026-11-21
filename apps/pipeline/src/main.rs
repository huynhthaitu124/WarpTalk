//! Realtime pipeline host. Owns translation rooms (Redis lease), pulls participant audio from
//! the media transport, runs VAD -> STT per speaker and publishes `stt:results` plus captions.
//!
//! Environment (see also `warptalk-config`): `REDIS_URL`, `DATABASE_URL` (optional: without it
//! the platform defaults of the `pipeline` settings namespace are used and never reloaded),
//! `METRICS_BIND`, `MEDIA_TRANSPORT=simulated|livekit`, `OPENAI_API_KEY`, `PIPELINE_HOST_ID`,
//! `PIPELINE_LEASE_TTL_MS`, `PIPELINE_SCAN_INTERVAL_MS`, `LIVEKIT_URL|API_KEY|API_SECRET`,
//! `SIM_PARTICIPANTS`, `SIM_WAV_DIR`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, bail};
use tokio::sync::watch;
use tracing::{info, warn};
use warptalk_config::{DatabaseConfig, LogFormat as ConfigLogFormat, RedisConfig, TelemetryConfig};
use warptalk_pipeline::config::{AppConfig, TransportKind, pipeline_config_from_settings};
use warptalk_pipeline::host::{HostConfig, HostDeps, PipelineHost};
use warptalk_pipeline::results::RedisResultSink;
use warptalk_pipeline::simulated::{SimParticipant, SimulatedTransport};
use warptalk_pipeline::transport::MediaTransport;
use warptalk_pipeline::SttOnlyStage;
use warptalk_pipeline_runtime::testing::{FakeStt, energy_vad_factory};
use warptalk_pipeline_runtime::{PipelineConfig, SpeechToText};
use warptalk_settings::{
    NS_PIPELINE, PipelineSettings, ScopeChain, SettingsStore, register_defaults,
};
use warptalk_stt::{OpenAiRealtimeStt, OpenAiSttConfig};
use warptalk_telemetry::{LogFormat, init_metrics, init_tracing};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = TelemetryConfig::from_env().context("telemetry config")?;
    let format = match telemetry.log_format {
        ConfigLogFormat::Json => LogFormat::Json,
        _ => LogFormat::Pretty,
    };
    init_tracing("warptalk-pipeline", format, &telemetry.log_filter).context("tracing")?;
    let metrics_bind = telemetry
        .metrics_bind
        .as_deref()
        .map(str::parse::<SocketAddr>)
        .transpose()
        .context("METRICS_BIND must be host:port")?;
    init_metrics(metrics_bind).context("metrics")?;

    let app = AppConfig::from_env().context("pipeline config")?;
    info!(config = ?app, "starting warptalk-pipeline");
    // Fail fast on transport problems, before touching Redis.
    let transport = build_transport(&app)?;

    let redis_cfg = RedisConfig::from_env().context("redis config")?;
    let redis = warptalk_bus::connect(&redis_cfg.url)
        .await
        .context("connecting to redis")?;

    let settings = load_settings(&redis_cfg, redis.clone()).await;
    let initial = settings.borrow().clone();
    let (cfg_tx, cfg_rx) = watch::channel(pipeline_config_from_settings(&initial));
    spawn_settings_bridge(settings, cfg_tx);

    let stt = build_stt(&app, &initial).await?;

    let host = PipelineHost::new(
        {
            let mut c = HostConfig::new(app.host_id.clone(), app.lease_ttl);
            c.scan_interval = app.scan_interval;
            c
        },
        HostDeps {
            redis: redis.clone(),
            redis_url: redis_cfg.url.clone(),
            transport,
            stt,
            vad_factory: energy_vad_factory(),
            downstream: Arc::new(SttOnlyStage),
            results: Arc::new(RedisResultSink::new(redis, redis_cfg.stream_maxlen)),
            config: cfg_rx,
        },
    );

    let (stop_tx, stop_rx) = watch::channel(false);
    let runner = {
        let host = host.clone();
        tokio::spawn(async move { host.run(stop_rx).await })
    };
    shutdown_signal().await;
    info!("shutdown requested, draining rooms");
    let _ = stop_tx.send(true);
    runner.await.context("host task")?;
    info!("bye");
    Ok(())
}

/// Settings from the store when `DATABASE_URL` is set, else fixed platform defaults.
async fn load_settings(
    redis_cfg: &RedisConfig,
    redis: redis::aio::ConnectionManager,
) -> watch::Receiver<PipelineSettings> {
    let fixed = || watch::channel(PipelineSettings::default()).1;
    let db = match DatabaseConfig::from_env() {
        Ok(db) => db,
        Err(_) => {
            warn!("DATABASE_URL not set: using default pipeline settings (no hot reload)");
            return fixed();
        }
    };
    let pool = match warptalk_db::connect(&db.url, db.max_connections).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "cannot connect to postgres: using default pipeline settings");
            return fixed();
        }
    };
    let store = SettingsStore::new(pool, Some(redis));
    register_defaults(&store);
    store.spawn_invalidation_listener(&redis_cfg.url);
    match store
        .watch::<PipelineSettings>(NS_PIPELINE, ScopeChain::platform())
        .await
    {
        Ok(rx) => rx,
        Err(e) => {
            warn!(error = %e, "cannot read pipeline settings: using defaults");
            fixed()
        }
    }
}

/// settings watch -> validated `PipelineConfig` watch that every room worker follows.
fn spawn_settings_bridge(
    mut settings: watch::Receiver<PipelineSettings>,
    tx: watch::Sender<PipelineConfig>,
) {
    tokio::spawn(async move {
        while settings.changed().await.is_ok() {
            let cfg = pipeline_config_from_settings(&settings.borrow_and_update());
            match cfg.validate() {
                Ok(()) => {
                    info!("pipeline settings changed, reloading");
                    let _ = tx.send(cfg);
                }
                Err(e) => warn!(error = %e, "ignoring invalid pipeline settings"),
            }
        }
    });
}

async fn build_stt(
    app: &AppConfig,
    settings: &PipelineSettings,
) -> anyhow::Result<Arc<dyn SpeechToText>> {
    if let Some(key) = &app.openai_api_key {
        let mut cfg = OpenAiSttConfig::new(key.clone());
        cfg.min_avg_logprob = settings.stt_min_avg_logprob;
        if let Some(model) = &app.openai_stt_model {
            cfg.model = model.clone();
        }
        let stt = Arc::new(OpenAiRealtimeStt::new(cfg));
        let warm = stt.warm_up().await;
        info!(warm_sockets = warm, "OpenAI realtime STT ready");
        return Ok(stt);
    }
    match app.transport {
        TransportKind::Simulated => {
            warn!("OPENAI_API_KEY not set: using the fake STT (simulated transport only)");
            Ok(Arc::new(FakeStt::default()))
        }
        TransportKind::LiveKit => bail!("OPENAI_API_KEY is required (no STT provider configured)"),
    }
}

fn build_transport(app: &AppConfig) -> anyhow::Result<Arc<dyn MediaTransport>> {
    match app.transport {
        TransportKind::Simulated => {
            let mut participants = Vec::new();
            for id in &app.sim_participants {
                let wav = app.sim_wav_dir.as_ref().map(|d| d.join(format!("{id}.wav")));
                let p = match wav.filter(|p| p.exists()) {
                    Some(path) => SimParticipant::from_wav(id.clone(), &path, true)
                        .with_context(|| format!("reading {}", path.display()))?,
                    None => SimParticipant::synthetic(id.clone(), 3_000, 2_500),
                };
                participants.push(p);
            }
            if participants.is_empty() {
                warn!(
                    "simulated transport without SIM_PARTICIPANTS: nobody speaks; \
                     set it to the participant ids used as route sources"
                );
            }
            Ok(Arc::new(SimulatedTransport::with_participants(participants)))
        }
        TransportKind::LiveKit => livekit_transport(app),
    }
}

#[cfg(feature = "livekit-rtc")]
fn livekit_transport(app: &AppConfig) -> anyhow::Result<Arc<dyn MediaTransport>> {
    let cfg = app.livekit.clone().context("LiveKit settings missing")?;
    Ok(Arc::new(warptalk_pipeline::livekit_transport::LiveKitTransport::new(cfg)))
}

#[cfg(not(feature = "livekit-rtc"))]
fn livekit_transport(_app: &AppConfig) -> anyhow::Result<Arc<dyn MediaTransport>> {
    bail!(
        "MEDIA_TRANSPORT=livekit needs a build with the `livekit-rtc` feature (it requires \
         clang >= 21 for webrtc-sys; see apps/pipeline/LIVEKIT.md). \
         Use MEDIA_TRANSPORT=simulated for local runs."
    )
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => warn!(error = %e, "cannot install SIGTERM handler"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
