//! Translation Room: lifecycle, lobby, participants, series, audio routes, sessions, bridge, artifacts, minutes (schema translation_room).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::{AppResult, Clock, RoomId, UserId, WorkspaceId};
use warptalk_web::JwtKeys;

use application::ports::{RoomEventPublisher, RoomRepo, VoiceConsentDirectory, WorkspacePolicy};
use application::service::RoomService;
use infrastructure::pg_repo::PgRoomRepo;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "room",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct RoomDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub workspace: Arc<dyn WorkspacePolicy>,
    pub consent: Arc<dyn VoiceConsentDirectory>,
    pub events: Arc<dyn RoomEventPublisher>,
}

pub struct RoomModule {
    service: Arc<RoomService>,
    repo: Arc<dyn RoomRepo>,
    jwt: JwtKeys,
}

impl RoomModule {
    pub fn new(deps: RoomDeps) -> Self {
        let repo: Arc<dyn RoomRepo> = Arc::new(PgRoomRepo::new(deps.pool, deps.consent));
        let service = Arc::new(RoomService::new(
            repo.clone(),
            deps.workspace,
            deps.events,
            deps.clock,
        ));
        Self {
            service,
            repo,
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

    /// Read-only facade for other modules.
    pub fn queries(&self) -> RoomQueries {
        RoomQueries {
            repo: self.repo.clone(),
        }
    }
}

/// Public query facade for other modules.
#[derive(Clone)]
pub struct RoomQueries {
    repo: Arc<dyn RoomRepo>,
}

impl RoomQueries {
    /// `COALESCE(active_host_id, host_id)`; `None` when the room does not exist.
    pub async fn effective_host(&self, room: RoomId) -> AppResult<Option<UserId>> {
        Ok(self.repo.get(room).await?.map(|r| r.effective_host()))
    }

    pub async fn room_workspace(&self, room: RoomId) -> AppResult<Option<WorkspaceId>> {
        Ok(self.repo.get(room).await?.map(|r| r.workspace_id))
    }
}
