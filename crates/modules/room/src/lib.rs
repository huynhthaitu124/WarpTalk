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

    /// Summary of a room for other modules; `None` when it does not exist.
    pub async fn room_summary(&self, room: RoomId) -> AppResult<Option<RoomSummary>> {
        Ok(self.repo.get(room).await?.map(|r| RoomSummary {
            workspace_id: r.workspace_id,
            host_id: r.host_id,
            effective_host_id: r.effective_host(),
            status: r.status.as_str().to_owned(),
        }))
    }

    /// Participant status of `user` in `room`; `None` when the user never joined.
    pub async fn participant_summary(
        &self,
        room: RoomId,
        user: UserId,
    ) -> AppResult<Option<ParticipantSummary>> {
        Ok(self
            .repo
            .participants(room)
            .await?
            .into_iter()
            .find(|p| p.user_id == Some(user))
            .map(|p| ParticipantSummary {
                status: p.status.as_str().to_owned(),
                display_name: p.display_name,
            }))
    }
}

/// Read-only room facts (`status` is the room status string, e.g. `IN_PROGRESS`).
#[derive(Debug, Clone)]
pub struct RoomSummary {
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    /// `COALESCE(active_host_id, host_id)`.
    pub effective_host_id: UserId,
    pub status: String,
}

/// Read-only participant facts (`status` is e.g. `WAITING`, `CONNECTED`).
#[derive(Debug, Clone)]
pub struct ParticipantSummary {
    pub status: String,
    pub display_name: String,
}
