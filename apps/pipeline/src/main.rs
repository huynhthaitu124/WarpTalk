//! Realtime pipeline host. Owns translation rooms (Redis lease), pulls participant audio from
//! the media transport, runs VAD -> STT -> MT -> TTS per speaker, publishes `stt:results`,
//! `translate:results`, `tts:results`, captions (with translations) and the dub audio.
//!
//! Environment (see also `warptalk-config`): `REDIS_URL`, `DATABASE_URL` (optional: without it
//! the platform defaults of the `pipeline` settings namespace are used and never reloaded),
//! `METRICS_BIND`, `MEDIA_TRANSPORT=simulated|livekit`, `OPENAI_API_KEY` (STT and MT),
//! `CARTESIA_API_KEY` (TTS), `PIPELINE_FAKE_PROVIDERS=1` (fake MT/TTS for local demos when the
//! keys are absent), `PIPELINE_HOST_ID`, `PIPELINE_LEASE_TTL_MS`, `PIPELINE_SCAN_INTERVAL_MS`,
//! `LIVEKIT_URL|API_KEY|API_SECRET`, `SIM_PARTICIPANTS`, `SIM_WAV_DIR`.
//!
//! The `pipeline` settings namespace carries the VAD/STT knobs (`PipelineSettings`) and the
//! keys `mt` (`MtConfig`), `tts` (`TtsConfig`) and `latency` (`LatencyTuning`); all hot-reload.
//! Without `OPENAI_API_KEY` (and without fake providers) the host runs STT only.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, bail};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tracing::{info, warn};
use warptalk_cartesia::{CartesiaClient, CartesiaConfig};
use warptalk_config::{DatabaseConfig, LogFormat as ConfigLogFormat, RedisConfig, TelemetryConfig};
use warptalk_llm::{ProviderConfig, ProviderId, ProviderRegistry};
use warptalk_mt::{MtConfig, NoContext, OpenAiTranslator, RealtimeClient};
use warptalk_pipeline::config::{AppConfig, TransportKind, pipeline_config};
use warptalk_pipeline::downstream::{DownstreamStage, Providers, SttOnlyStage, TranslateDubStage};
use warptalk_pipeline::fakes::{FakeMt, FakeMtConfig, FakeTts, FakeTtsConfig};
use warptalk_pipeline::glossary::{RedisGlossaryProvider, RedisKv, spawn_invalidation_listener};
use warptalk_pipeline::host::{HostConfig, HostDeps, PipelineHost};
use warptalk_pipeline::results::RedisResultSink;
use warptalk_pipeline::simulated::{SimParticipant, SimulatedTransport};
use warptalk_pipeline::transport::MediaTransport;
use warptalk_pipeline::tuning::{LatencyTuning, PipelineExtras};
use warptalk_pipeline_runtime::testing::{FakeStt, energy_vad_factory};
use warptalk_pipeline_runtime::{PipelineConfig, SpeechToText};
use warptalk_settings::{
    NS_PIPELINE, PipelineSettings, ScopeChain, SettingsStore, Validate, register_defaults,
};
use warptalk_stt::{OpenAiRealtimeStt, OpenAiSttConfig};
use warptalk_telemetry::{LogFormat, init_metrics, init_tracing};
use warptalk_tts::testing::{FakeCatalog, FakeCloner};
use warptalk_tts::{
    CartesiaCatalog, CartesiaSynthesizer, CartesiaVoiceCloner, CatalogVoice, CloneRequest,
    InMemoryTtsCache, RedisTtsCache, Synthesizer, TtsCache, TtsConfig, TtsError,
    VoiceCatalogProvider, VoiceCloner,
};

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

    let store = connect_store(&redis_cfg, redis.clone()).await;
    let settings = watch_namespace::<PipelineSettings>(&store).await;
    let extras = watch_namespace::<PipelineExtras>(&store).await;
    let initial = settings.borrow().clone();
    let initial_extras = extras.borrow().clone();
    let (cfg_tx, cfg_rx) = watch::channel(pipeline_config(&initial, &initial_extras.latency));
    spawn_settings_bridge(settings, extras.clone(), cfg_tx);
    let (mt_rx, tts_rx, tuning_rx) = spawn_extras_bridge(extras, app.openai_api_key.clone());

    let stt = build_stt(&app, &initial).await?;
    let glossary = Arc::new(RedisGlossaryProvider::new(Arc::new(RedisKv(redis.clone()))));
    spawn_invalidation_listener(glossary.clone(), redis_cfg.url.clone());
    let providers = build_providers(&app, &redis, &mt_rx, glossary).await?;
    let downstream: Arc<dyn DownstreamStage> = match providers {
        Some(p) => {
            info!("chain: STT -> MT -> TTS -> dub");
            Arc::new(TranslateDubStage::assemble(p, mt_rx, tts_rx, tuning_rx).0)
        }
        None => {
            warn!(
                "no MT provider (set OPENAI_API_KEY, or PIPELINE_FAKE_PROVIDERS=1 for a demo): \
                 running STT only"
            );
            Arc::new(SttOnlyStage)
        }
    };

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
            downstream,
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

/// The settings store when `DATABASE_URL` is set and reachable.
async fn connect_store(
    redis_cfg: &RedisConfig,
    redis: redis::aio::ConnectionManager,
) -> Option<SettingsStore> {
    let db = match DatabaseConfig::from_env() {
        Ok(db) => db,
        Err(_) => {
            warn!("DATABASE_URL not set: using default pipeline settings (no hot reload)");
            return None;
        }
    };
    let pool = match warptalk_db::connect(&db.url, db.max_connections).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "cannot connect to postgres: using default pipeline settings");
            return None;
        }
    };
    let store = SettingsStore::new(pool, Some(redis));
    register_defaults(&store);
    store.spawn_invalidation_listener(&redis_cfg.url);
    Some(store)
}

/// A hot-reloading view of the `pipeline` namespace as `T`; fixed defaults without a store.
async fn watch_namespace<T>(store: &Option<SettingsStore>) -> watch::Receiver<T>
where
    T: DeserializeOwned + Default + Validate + PartialEq + Send + Sync + 'static,
{
    if let Some(store) = store {
        match store.watch::<T>(NS_PIPELINE, ScopeChain::platform()).await {
            Ok(rx) => return rx,
            Err(e) => warn!(error = %e, "cannot read pipeline settings: using defaults"),
        }
    }
    watch::channel(T::default()).1
}

/// settings + latency flags -> validated `PipelineConfig` watch that every room worker follows.
fn spawn_settings_bridge(
    mut settings: watch::Receiver<PipelineSettings>,
    mut extras: watch::Receiver<PipelineExtras>,
    tx: watch::Sender<PipelineConfig>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                r = settings.changed() => if r.is_err() { break },
                r = extras.changed() => if r.is_err() { break },
            }
            let cfg = pipeline_config(
                &settings.borrow_and_update(),
                &extras.borrow_and_update().latency,
            );
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

/// `mt`, `tts` and `latency` keys -> the three watches the downstream stage follows. The
/// realtime MT key comes from `OPENAI_API_KEY` unless the settings carry one.
fn spawn_extras_bridge(
    mut extras: watch::Receiver<PipelineExtras>,
    openai_key: Option<String>,
) -> (
    watch::Receiver<MtConfig>,
    watch::Receiver<TtsConfig>,
    watch::Receiver<LatencyTuning>,
) {
    let with_key = move |e: &PipelineExtras| {
        let mut mt = e.mt.clone();
        if mt.realtime.api_key.is_empty()
            && let Some(k) = &openai_key
        {
            mt.realtime.api_key.clone_from(k);
        }
        mt
    };
    let init = extras.borrow().clone();
    let (mt_tx, mt_rx) = watch::channel(with_key(&init));
    let (tts_tx, tts_rx) = watch::channel(init.tts.clone());
    let (tu_tx, tu_rx) = watch::channel(init.latency.clone());
    tokio::spawn(async move {
        while extras.changed().await.is_ok() {
            let e = extras.borrow_and_update().clone();
            mt_tx.send_if_modified(|c| {
                let next = with_key(&e);
                (*c != next).then(|| *c = next).is_some()
            });
            tts_tx.send_if_modified(|c| (*c != e.tts).then(|| *c = e.tts.clone()).is_some());
            tu_tx.send_if_modified(|c| (*c != e.latency).then(|| *c = e.latency.clone()).is_some());
        }
    });
    (mt_rx, tts_rx, tu_rx)
}

/// Stock voices unavailable: the stage then relies on the built-in default voice.
struct NoCatalog;

#[async_trait]
impl VoiceCatalogProvider for NoCatalog {
    async fn voices(&self, _language: &str) -> Vec<CatalogVoice> {
        Vec::new()
    }
    fn default_voice(&self, _language: &str) -> String {
        "default".into()
    }
}

struct NoCloner;

#[async_trait]
impl VoiceCloner for NoCloner {
    async fn clone_voice(&self, _req: &CloneRequest) -> Result<String, TtsError> {
        Err(TtsError::Permanent("voice cloning not configured".into()))
    }
    async fn delete_voice(&self, _voice_id: &str) -> Result<(), TtsError> {
        Ok(())
    }
}

/// The Cartesia-backed TTS pieces, or inert ones when no key is configured.
struct TtsParts {
    synthesizer: Option<Arc<dyn Synthesizer>>,
    catalog: Arc<dyn VoiceCatalogProvider>,
    cloner: Arc<dyn VoiceCloner>,
}

async fn build_tts(app: &AppConfig) -> anyhow::Result<TtsParts> {
    let Some(key) = &app.cartesia_api_key else {
        warn!("CARTESIA_API_KEY not set: translated captions only, no dub");
        return Ok(TtsParts {
            synthesizer: None,
            catalog: Arc::new(NoCatalog),
            cloner: Arc::new(NoCloner),
        });
    };
    let client = Arc::new(
        CartesiaClient::new(CartesiaConfig::new(key.clone()))
            .context("building the Cartesia client")?,
    );
    let warm = client.warm_up().await;
    info!(warm_sockets = warm, "Cartesia TTS ready");
    Ok(TtsParts {
        synthesizer: Some(Arc::new(CartesiaSynthesizer::new(client.clone()))),
        catalog: Arc::new(CartesiaCatalog::new(client.clone())),
        cloner: Arc::new(CartesiaVoiceCloner::new(client)),
    })
}

/// Real providers when their keys are present, fakes behind `PIPELINE_FAKE_PROVIDERS`, else
/// `None` (STT only). MT needs `OPENAI_API_KEY`; the dub additionally needs `CARTESIA_API_KEY`
/// (without it the chain stops at translated captions).
async fn build_providers(
    app: &AppConfig,
    redis: &redis::aio::ConnectionManager,
    mt_rx: &watch::Receiver<MtConfig>,
    glossary: Arc<RedisGlossaryProvider>,
) -> anyhow::Result<Option<Providers>> {
    let cache = |real: bool| -> Arc<dyn TtsCache> {
        if real {
            Arc::new(RedisTtsCache::new(redis.clone()))
        } else {
            Arc::new(InMemoryTtsCache::new(4096))
        }
    };
    if let Some(key) = &app.openai_api_key {
        let llm = ProviderRegistry::build(ProviderConfig::new(ProviderId::OpenAi, key.clone()))
            .context("building the OpenAI chat provider")?;
        let mt_cfg = mt_rx.borrow().clone();
        let realtime = if mt_cfg.realtime.enabled {
            let rt = RealtimeClient::new(mt_cfg.realtime.clone());
            let warm = rt.warm_up().await;
            info!(warm_sockets = warm, "OpenAI realtime MT ready");
            Some(rt)
        } else {
            None
        };
        let translator = Arc::new(OpenAiTranslator::new(mt_rx.clone(), realtime, llm));
        let tts = build_tts(app).await?;
        return Ok(Some(Providers {
            translator,
            glossary: glossary.clone(),
            context: Arc::new(NoContext),
            synthesizer: tts.synthesizer,
            catalog: tts.catalog,
            cloner: tts.cloner,
            tts_cache: cache(true),
        }));
    }
    if app.fake_providers {
        if app.transport != TransportKind::Simulated {
            bail!("PIPELINE_FAKE_PROVIDERS is for MEDIA_TRANSPORT=simulated only");
        }
        warn!("PIPELINE_FAKE_PROVIDERS: fake MT and TTS with the spec 08 latency profile");
        return Ok(Some(Providers {
            translator: FakeMt::new(FakeMtConfig::default()),
            glossary: glossary.clone(),
            context: Arc::new(NoContext),
            synthesizer: Some(FakeTts::new(FakeTtsConfig::default())),
            catalog: Arc::new(FakeCatalog(vec!["voice-a".into(), "voice-b".into()])),
            cloner: Arc::new(FakeCloner::default()),
            tts_cache: cache(false),
        }));
    }
    Ok(None)
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
                let wav = app
                    .sim_wav_dir
                    .as_ref()
                    .map(|d| d.join(format!("{id}.wav")));
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
            Ok(Arc::new(SimulatedTransport::with_participants(
                participants,
            )))
        }
        TransportKind::LiveKit => livekit_transport(app),
    }
}

#[cfg(feature = "livekit-rtc")]
fn livekit_transport(app: &AppConfig) -> anyhow::Result<Arc<dyn MediaTransport>> {
    let cfg = app.livekit.clone().context("LiveKit settings missing")?;
    Ok(Arc::new(
        warptalk_pipeline::livekit_transport::LiveKitTransport::new(cfg),
    ))
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
