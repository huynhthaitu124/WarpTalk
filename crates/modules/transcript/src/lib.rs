//! Segments, corrections, translation contents, dubbing records, glossaries, exports, finalize (schema transcript).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.
//!
//! Slice 2: workspace and global glossaries with Redis snapshots for the pipeline.
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

use application::glossary_ports::{DenyAllRoles, GlossaryFeed, GlossaryRepo, WorkspaceRoles};
use application::glossary_service::GlossaryService;
use application::ingest::IngestService;
use application::ports::{
    RoomDirectory, RoomHost, RoomPolicy, TranscriptReadAccess, TranscriptRepo, WorkspaceAiPolicy,
};
use application::service::TranscriptService;
use infrastructure::consumers::{SttResultsHandler, TranslateResultsHandler, TtsResultsHandler};
use infrastructure::pg_glossary_repo::PgGlossaryRepo;
use infrastructure::pg_repo::PgTranscriptRepo;
use infrastructure::redis_adapters::{RedisEmbeddingPublisher, RedisGlossaryFeed, RedisPauseGate};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "transcript",
    migrations: &[
        Migration {
            version: 1,
            name: "init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "glossaries",
            sql: include_str!("../migrations/0002_glossaries.sql"),
        },
    ],
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
    glossaries: Arc<GlossaryService>,
    glossary_repo: Arc<dyn GlossaryRepo>,
    glossary_feed: Arc<dyn GlossaryFeed>,
    clock: Arc<dyn Clock>,
    jwt: JwtKeys,
}

impl TranscriptModule {
    pub fn new(deps: TranscriptDeps) -> Self {
        let glossary_repo: Arc<dyn GlossaryRepo> = Arc::new(PgGlossaryRepo::new(deps.pool.clone()));
        let feed: Arc<dyn GlossaryFeed> = Arc::new(RedisGlossaryFeed::new(deps.redis.clone()));
        let glossaries = Arc::new(GlossaryService::new(
            glossary_repo.clone(),
            Arc::new(DenyAllRoles),
            feed.clone(),
            deps.clock.clone(),
        ));
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
            deps.clock.clone(),
        ));
        Self {
            service,
            ingest,
            glossaries,
            glossary_repo,
            glossary_feed: feed,
            clock: deps.clock,
            jwt: deps.jwt,
        }
    }

    /// Wires workspace membership/roles for the glossary endpoints. Without it every glossary
    /// request is answered 403 (nobody is a member).
    pub fn with_workspace_roles(mut self, roles: Arc<dyn WorkspaceRoles>) -> Self {
        self.glossaries = Arc::new(GlossaryService::new(
            self.glossary_repo.clone(),
            roles,
            self.glossary_feed.clone(),
            self.clock.clone(),
        ));
        self
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::AppState {
            service: self.service.clone(),
            jwt: self.jwt.clone(),
        })
        .merge(http::glossary::router(http::glossary::GlossaryState {
            service: self.glossaries.clone(),
            jwt: self.jwt.clone(),
        }))
    }

    /// Glossary use cases (exposed for tests and tooling).
    pub fn glossaries(&self) -> Arc<GlossaryService> {
        self.glossaries.clone()
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
