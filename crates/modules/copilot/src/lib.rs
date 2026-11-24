//! Copilot sessions for external meetings (ADR 0001): session lifecycle, transcript segment
//! ingest, ephemeral STT tokens (schema `copilot`).
//!
//! A session is a room of type `COPILOT_SESSION` owned by the room module (created through the
//! [`application::ports::SessionRooms`] port), so transcripts, follow-ups, suggestions and
//! access checks keep working keyed by the room id. This module owns the facts specific to a
//! session (platform, languages, speaker names) and writes segments to `stt:results` in the
//! pipeline's own contract.
//!
//! Layers: `domain` (pure rules), `application` (use cases and ports), `infrastructure`
//! (sqlx, Redis, OpenAI adapters), `http` (axum handlers and DTOs).

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_web::JwtKeys;

use application::ports::{
    RateLimiter, SegmentSink, SessionRepo, SessionRooms, SttTokenIssuer, WorkspaceMembers,
};
use application::service::CopilotService;
use infrastructure::pg_repo::PgSessionRepo;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "copilot",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct CopilotDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub rooms: Arc<dyn SessionRooms>,
    pub members: Arc<dyn WorkspaceMembers>,
    /// Writes `stt:results`.
    pub sink: Arc<dyn SegmentSink>,
    pub stt_tokens: Arc<dyn SttTokenIssuer>,
    pub rate_limiter: Arc<dyn RateLimiter>,
}

pub struct CopilotModule {
    service: Arc<CopilotService>,
    jwt: JwtKeys,
}

impl CopilotModule {
    pub fn new(deps: CopilotDeps) -> Self {
        let repo: Arc<dyn SessionRepo> = Arc::new(PgSessionRepo::new(deps.pool));
        let service = Arc::new(CopilotService::new(
            repo,
            deps.rooms,
            deps.members,
            deps.sink,
            deps.stt_tokens,
            deps.rate_limiter,
            deps.clock,
        ));
        Self {
            service,
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
}
