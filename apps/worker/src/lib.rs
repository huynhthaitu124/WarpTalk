//! Async plane: Redis Streams consumers (transcript persistence now; billing metering,
//! embeddings, summaries, suggestions, notifications later).

pub mod adapters;
pub mod metering;
pub mod route_status;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage, run_consumer};
use warptalk_config::JwtConfig;
use warptalk_kernel::SystemClock;
use warptalk_transcript::application::ports::{AlwaysSaveRoomPolicy, NoopReadAccess, NoopRoomHost};
use warptalk_transcript::{TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;

use crate::adapters::{
    AiChatTranslator, PgChatTargetLanguages, PgRoomDirectory, PgRoomMetaSource,
    PgSuggestionRoomDirectory, PgTranscriptSource, PgWorkspaceAiPolicy,
};
use redis::aio::ConnectionManager;
use warptalk_ai_plane::ai::AiProviders;
use warptalk_ai_plane::knowledge::{
    DenyDocumentAccess, KnowledgeParts, ServiceRetriever, build_knowledge_module,
    document_source_from_env,
};
use warptalk_assistant::suggestion::LlmResolver;
use warptalk_assistant::suggestion::{
    EngineDeps, RedisConsentPolicy, RedisRoomSignals, SettingsConfigSource, SuggestionEngine,
    SuggestionState, SuggestionWorker,
};
use warptalk_assistant::summary::{
    PgSummaryRepository, RedisFollowUpSource, RedisSummaryBus, SummaryDeps, SummaryService,
    SummaryWorker,
};
use warptalk_knowledge::infrastructure::{INDEX_RESULTS_STREAM, RedisResultPublisher};
use warptalk_knowledge::{KnowledgeModule, index_consumer_config, purge_consumer_config};
use warptalk_meeting::infrastructure::{
    ChatTranslateHandler, RedisChatEventSink, chat_translate_consumer_config,
};
use warptalk_qdrant::VectorStore;
use warptalk_settings::{AiProvidersSettings, NS_AI_PROVIDERS, ScopeChain, SettingsStore};
use warptalk_workspace::{
    NoopUserDirectory, WorkspaceDeps, WorkspaceModule, document_results_consumer_config,
};

/// Model pipelines the suggestion worker runs at once across rooms.
const SUGGESTION_MAX_PIPELINES: usize = 8;

/// Summary jobs a worker process runs at once.
const SUMMARY_MAX_JOBS: usize = 4;

/// How long shutdown waits for consumers to stop before giving up.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Keys for modules that never serve HTTP in this process.
pub(crate) fn dummy_jwt() -> JwtKeys {
    JwtKeys::from_config(&JwtConfig {
        secret: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
        issuer: "warptalk-worker".into(),
        audience: "warptalk-worker".into(),
        access_ttl_seconds: 1,
        refresh_ttl_seconds: 1,
    })
}

/// Build the transcript module with the worker's simple adapters.
pub async fn build_transcript_module(
    pool: PgPool,
    redis_url: &str,
) -> anyhow::Result<TranscriptModule> {
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    // The worker mounts no HTTP routes; the module only needs keys to be constructible.
    let jwt = dummy_jwt();
    Ok(TranscriptModule::new(TranscriptDeps {
        pool: pool.clone(),
        redis,
        jwt,
        clock: Arc::new(SystemClock),
        rooms: Arc::new(PgRoomDirectory::new(pool.clone())),
        // TODO: replace with the room module's SaveTranscript setting (spec 12 §5.4, WT-587).
        room_policy: Arc::new(AlwaysSaveRoomPolicy),
        workspace_ai: Arc::new(PgWorkspaceAiPolicy::new(pool)),
        host: Arc::new(NoopRoomHost),
        read_access: Arc::new(NoopReadAccess),
    }))
}

/// Spawn one `run_consumer` task per consumer, each on its own Redis connection
/// (XREADGROUP BLOCK must not share a multiplexed connection with other commands).
pub async fn spawn_consumers(
    redis_url: &str,
    consumers: Vec<(ConsumerConfig, Arc<dyn StreamHandler>)>,
    shutdown: &watch::Receiver<bool>,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::with_capacity(consumers.len());
    for (cfg, handler) in consumers {
        let conn = warptalk_bus::connect(redis_url)
            .await
            .context("connecting consumer to Redis")?;
        handles.push(tokio::spawn(run_consumer(
            conn,
            cfg,
            handler,
            shutdown.clone(),
        )));
    }
    Ok(handles)
}

/// Wait for all consumer tasks, bounded by the shutdown grace period.
pub async fn join_consumers(handles: Vec<JoinHandle<()>>) {
    let all = async {
        for h in handles {
            if let Err(e) = h.await {
                tracing::error!(error = %e, "consumer task failed");
            }
        }
    };
    if tokio::time::timeout(SHUTDOWN_GRACE, all).await.is_err() {
        tracing::warn!("consumers did not stop within the grace period");
    }
}

/// Knowledge consumers (indexing, purge) and the live suggestion worker.
pub struct AiPlane {
    pub knowledge: KnowledgeModule,
    pub suggestion: Arc<SuggestionWorker>,
    pub ai: Arc<AiProviders>,
}

/// Build the knowledge module and the suggestion worker with the worker's adapters.
pub async fn build_ai_plane(
    pool: PgPool,
    redis_url: &str,
    vectors: Arc<dyn VectorStore>,
    ai: Arc<AiProviders>,
    settings: SettingsStore,
) -> anyhow::Result<AiPlane> {
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    let knowledge = build_knowledge_module(KnowledgeParts {
        pool: pool.clone(),
        jwt: dummy_jwt(),
        settings: settings.clone(),
        vectors,
        ai: ai.clone(),
        access: Arc::new(DenyDocumentAccess),
        results: Arc::new(RedisResultPublisher::new(
            redis.clone(),
            INDEX_RESULTS_STREAM,
        )),
        documents: document_source_from_env(),
    })
    .await;

    warptalk_assistant::suggestion::register_config(&settings);
    let usage = Arc::new(warptalk_assistant::usage::RedisUsageSink::new(
        redis.clone(),
    ));
    let engine = SuggestionEngine::new(EngineDeps {
        rooms: Arc::new(PgSuggestionRoomDirectory::new(pool.clone())),
        signals: Arc::new(RedisRoomSignals::new(redis.clone())),
        consent: Arc::new(RedisConsentPolicy::new(redis.clone())),
        llm: ai.clone(),
        knowledge: Arc::new(ServiceRetriever::new(knowledge.service())),
        config: Arc::new(SettingsConfigSource::new(settings)),
        clock: Arc::new(SystemClock),
        state: SuggestionState::new(redis.clone()),
        redis,
    })
    .with_usage(usage)
    .with_store(Arc::new(
        warptalk_assistant::suggestion::PgSuggestionStore::new(pool.clone()),
    ));
    let suggestion = Arc::new(SuggestionWorker::new(
        Arc::new(engine),
        SUGGESTION_MAX_PIPELINES,
    ));
    Ok(AiPlane {
        knowledge,
        suggestion,
        ai,
    })
}

/// Post-session summary worker: transcript and room adapters, the platform LLM resolver,
/// live follow-ups from Redis. Registers the `summary` settings namespace.
pub async fn build_summary(
    pool: PgPool,
    redis_url: &str,
    llm: Arc<dyn LlmResolver>,
    settings: &SettingsStore,
) -> anyhow::Result<Arc<SummaryWorker>> {
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    warptalk_assistant::summary::register_config(settings);
    let service = SummaryService::new(SummaryDeps {
        repo: PgSummaryRepository::new(pool.clone()),
        transcripts: Arc::new(PgTranscriptSource::new(pool.clone())),
        rooms: Arc::new(PgRoomMetaSource::new(pool.clone())),
        policy: Arc::new(warptalk_assistant::summary::PgExternalLlmGate::new(pool)),
        followups: Arc::new(RedisFollowUpSource::new(redis.clone())),
        llm,
        settings: settings.clone(),
        bus: Arc::new(RedisSummaryBus::new(redis.clone())),
        clock: Arc::new(SystemClock),
    })
    .with_usage(Arc::new(warptalk_assistant::usage::RedisUsageSink::new(
        redis,
    )));
    Ok(Arc::new(SummaryWorker::new(
        Arc::new(service),
        SUMMARY_MAX_JOBS,
    )))
}

/// Settings store for the worker: platform defaults registered, invalidation listener running.
pub fn build_settings(pool: PgPool, redis: ConnectionManager, redis_url: &str) -> SettingsStore {
    let settings = SettingsStore::new(pool, Some(redis));
    warptalk_settings::namespaces::register_defaults(&settings);
    settings.spawn_invalidation_listener(redis_url);
    settings
}

/// Consumer list for the knowledge module (indexing + purge).
pub fn knowledge_consumers(k: &KnowledgeModule) -> Vec<(ConsumerConfig, Arc<dyn StreamHandler>)> {
    vec![
        (index_consumer_config(), k.index_handler()),
        (purge_consumer_config(), k.purge_handler()),
    ]
}

/// Consumer of `embedding:index_results` that updates workspace document status
/// (indexed / blocked / failed / deleted), group `workspace-document-embedding-results`.
pub fn workspace_document_consumer(pool: PgPool) -> (ConsumerConfig, Arc<dyn StreamHandler>) {
    // The worker mounts no HTTP routes: keys and user directory only make the module constructible.
    let workspace = WorkspaceModule::new(WorkspaceDeps {
        pool,
        jwt: dummy_jwt(),
        clock: Arc::new(SystemClock),
        users: Arc::new(NoopUserDirectory),
    });
    (
        document_results_consumer_config(),
        workspace.document_results_handler(),
    )
}

/// Start the suggestion worker on its own Redis connection.
pub async fn spawn_suggestion(
    redis_url: &str,
    worker: Arc<SuggestionWorker>,
    shutdown: &watch::Receiver<bool>,
) -> anyhow::Result<JoinHandle<()>> {
    let conn = warptalk_bus::connect(redis_url)
        .await
        .context("connecting suggestion consumer to Redis")?;
    Ok(worker.spawn(conn, shutdown.clone()))
}

/// Acks chat translation requests without translating (no AI key configured).
pub struct DiscardChatTranslations {
    logged: std::sync::atomic::AtomicBool,
}

impl DiscardChatTranslations {
    pub fn new() -> Self {
        Self {
            logged: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Default for DiscardChatTranslations {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StreamHandler for DiscardChatTranslations {
    async fn handle(&self, _: &StreamMessage) -> Result<(), HandlerError> {
        if !self.logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("no AI key configured: chat messages are not translated");
        }
        Ok(())
    }
}

/// Chat translation consumer (`translate:chat_requests`, group `meeting-chat-translate`).
/// Without an AI key the handler only acks.
pub async fn chat_translate_consumer(
    pool: PgPool,
    redis_url: &str,
    ai: &Arc<AiProviders>,
    settings: &SettingsStore,
) -> anyhow::Result<(ConsumerConfig, Arc<dyn StreamHandler>)> {
    let cfg = chat_translate_consumer_config();
    let Some(llm) = ai.chat() else {
        return Ok((cfg, Arc::new(DiscardChatTranslations::new())));
    };
    let model = settings
        .get::<AiProvidersSettings>(NS_AI_PROVIDERS, ScopeChain::platform())
        .await
        .unwrap_or_default()
        .translation
        .model;
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    let handler = ChatTranslateHandler::new(
        pool.clone(),
        Arc::new(PgChatTargetLanguages::new(pool)),
        Arc::new(AiChatTranslator::new(llm, model)),
        Arc::new(RedisChatEventSink::new(redis)),
        Arc::new(SystemClock),
    );
    Ok((cfg, Arc::new(handler)))
}
