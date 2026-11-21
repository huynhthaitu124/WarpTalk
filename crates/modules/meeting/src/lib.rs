//! LiveKit runtime: join/bridge tokens, webhooks, host media controls, recording egress, in-meeting chat (schema meeting).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.
//!
//! Slice 1: join gate + token, bridge token, end meeting, LiveKit webhooks (roster, tracks).

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::Clock;
use warptalk_livekit::LiveKitConfig;
use warptalk_web::JwtKeys;

use application::ports::{MeetingEventPublisher, MeetingRepo, RoomDirectory};
use application::service::MeetingService;
use infrastructure::pg_repo::PgMeetingRepo;

pub use warptalk_livekit::RoomService;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "meeting",
    migrations: &[
        Migration {
            version: 1,
            name: "init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "chat",
            sql: include_str!("../migrations/0002_chat.sql"),
        },
    ],
};

/// Participant token lifetime (spec: 11 §8, code constant 4 h).
pub const DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(4 * 3600);

pub struct MeetingDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub livekit: LiveKitConfig,
    pub rooms: Arc<dyn RoomDirectory>,
    pub room_service: Arc<dyn RoomService>,
    pub events: Arc<dyn MeetingEventPublisher>,
    pub token_ttl: Duration,
}

pub struct MeetingModule {
    service: Arc<MeetingService>,
    jwt: JwtKeys,
}

impl MeetingModule {
    pub fn new(deps: MeetingDeps) -> Self {
        let repo: Arc<dyn MeetingRepo> = Arc::new(PgMeetingRepo::new(deps.pool));
        let service = Arc::new(MeetingService::new(
            repo,
            deps.rooms,
            deps.room_service,
            deps.events,
            deps.clock,
            deps.livekit,
            deps.token_ttl,
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
