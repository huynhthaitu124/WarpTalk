//! Segments, corrections, translation contents, dubbing records, glossaries, exports, finalize (schema transcript).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.
//!
//! Slice 1: stream ingestion (stt/translate/tts results), read, corrections, finalize.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use redis::aio::ConnectionManager;
use sqlx::PgPool;
use warptalk_bus::{ConsumerConfig, StreamHandler};
use warptalk_contracts::streams::{self, groups};
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_web::JwtKeys;

use application::ingest::IngestService;
use application::ports::{
    RoomDirectory, RoomHost, RoomPolicy, TranscriptReadAccess, TranscriptRepo, WorkspaceAiPolicy,
};
use application::service::TranscriptService;
use infrastructure::consumers::{SttResultsHandler, TranslateResultsHandler, TtsResultsHandler};
use infrastructure::pg_repo::PgTranscriptRepo;
use infrastructure::redis_adapters::{RedisEmbeddingPublisher, RedisPauseGate};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "transcript",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct TranscriptDeps {
    pub pool: PgPool,
    pub redis: ConnectionManager,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub rooms: Arc<dyn RoomDirectory>,
    pub room_policy: Arc<dyn RoomPolicy>,
    pub workspace_ai: Arc<dyn WorkspaceAiPolicy>,
    pub host: Arc<dyn RoomHost>,
    pub read_access: Arc<dyn TranscriptReadAccess>,
}

pub struct TranscriptModule {
    service: Arc<TranscriptService>,
    ingest: Arc<IngestService>,
    jwt: JwtKeys,
}

impl TranscriptModule {
    pub fn new(deps: TranscriptDeps) -> Self {
        let repo: Arc<dyn TranscriptRepo> = Arc::new(PgTranscriptRepo::new(deps.pool));
        let ingest = Arc::new(IngestService::new(
            repo.clone(),
            deps.rooms,
            deps.room_policy,
            deps.workspace_ai,
            Arc::new(RedisPauseGate::new(deps.redis.clone())),
            Arc::new(RedisEmbeddingPublisher::new(deps.redis)),
            deps.clock.clone(),
        ));
        let service = Arc::new(TranscriptService::new(
            repo,
            deps.host,
            deps.read_access,
            deps.clock,
        ));
        Self {
            service,
            ingest,
            jwt: deps.jwt,
        }
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::AppState {
            service: self.service.clone(),
            jwt: self.jwt.clone(),
        })
    }

    /// Ingestion service (exposed for tests and tooling).
    pub fn ingest(&self) -> Arc<IngestService> {
        self.ingest.clone()
    }

    /// Consumers of group `transcript-persistence` (spec 12 §6.1); dead letter
    /// `{stream}:transcript-persistence:dead-letter`. Spawn each with `warptalk_bus::run_consumer`.
    pub fn consumers(&self) -> Vec<(ConsumerConfig, Arc<dyn StreamHandler>)> {
        let stt: Arc<dyn StreamHandler> = Arc::new(SttResultsHandler::new(self.ingest.clone()));
        let tr: Arc<dyn StreamHandler> =
            Arc::new(TranslateResultsHandler::new(self.ingest.clone()));
        let tts: Arc<dyn StreamHandler> = Arc::new(TtsResultsHandler::new(self.ingest.clone()));
        let cfg = |stream: &str| ConsumerConfig::new(stream, groups::TRANSCRIPT_PERSISTENCE);
        vec![
            (cfg(streams::STT_RESULTS), stt),
            (cfg(streams::TRANSLATE_RESULTS), tr.clone()),
            // Backfill results take the same path (spec 12 §6.1).
            (cfg(streams::TRANSLATE_BACKFILL_RESULTS), tr),
            (cfg(streams::TTS_RESULTS), tts),
        ]
    }
}
