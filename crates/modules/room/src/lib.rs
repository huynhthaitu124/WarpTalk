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

use application::ports::{
    LogInviteNotifier, RoomEventPublisher, RoomInviteNotifier, RoomRepo, VoiceConsentDirectory,
    WorkspacePolicy,
};
use application::service::RoomService;
pub use domain::BridgeConfig;
use infrastructure::pg_repo::PgRoomRepo;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "room",
    migrations: &[
        Migration {
            version: 1,
            name: "init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "external_bridge",
            sql: include_str!("../migrations/0002_external_bridge.sql"),
        },
        Migration {
            version: 3,
            name: "room_invitations",
            sql: include_str!("../migrations/0003_room_invitations.sql"),
        },
    ],
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
    clock: Arc<dyn Clock>,
    workspace: Arc<dyn WorkspacePolicy>,
    events: Arc<dyn RoomEventPublisher>,
    bridge: BridgeConfig,
    invites: Arc<dyn RoomInviteNotifier>,
}

impl RoomModule {
    pub fn new(deps: RoomDeps) -> Self {
        let repo: Arc<dyn RoomRepo> = Arc::new(PgRoomRepo::new(deps.pool, deps.consent));
        let bridge = BridgeConfig::default();
        let invites: Arc<dyn RoomInviteNotifier> = Arc::new(LogInviteNotifier);
        let service = Arc::new(
            RoomService::new(
                repo.clone(),
                deps.workspace.clone(),
                deps.events.clone(),
                deps.clock.clone(),
                bridge,
            )
            .with_invite_notifier(invites.clone()),
        );
        Self {
            service,
            repo,
            jwt: deps.jwt,
            clock: deps.clock,
            workspace: deps.workspace,
            events: deps.events,
            bridge,
            invites,
        }
    }

    /// Override the bridge timings (defaults: heartbeat 15 s, stale after 45 s).
    pub fn with_bridge_config(mut self, cfg: BridgeConfig) -> Self {
        self.bridge = cfg;
        self.rebuild_service();
        self
    }

    /// Override the outbound invitation notifier (default: log only, no email).
    pub fn with_invite_notifier(mut self, n: Arc<dyn RoomInviteNotifier>) -> Self {
        self.invites = n;
        self.rebuild_service();
        self
    }

    fn rebuild_service(&mut self) {
        self.service = Arc::new(
            RoomService::new(
                self.repo.clone(),
                self.workspace.clone(),
                self.events.clone(),
                self.clock.clone(),
                self.bridge,
            )
            .with_invite_notifier(self.invites.clone()),
        );
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
            service: self.service.clone(),
            repo: self.repo.clone(),
            clock: self.clock.clone(),
            bridge: self.bridge,
        }
    }
}

/// Public query facade for other modules.
#[derive(Clone)]
pub struct RoomQueries {
    service: Arc<RoomService>,
    repo: Arc<dyn RoomRepo>,
    clock: Arc<dyn Clock>,
    bridge: BridgeConfig,
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

    /// Bridge facts of an `EXTERNAL_BRIDGE` room; `None` for native or unknown rooms.
    pub async fn bridge_info(&self, room: RoomId) -> AppResult<Option<BridgeInfo>> {
        let now = self.clock.now();
        Ok(self.repo.get(room).await?.and_then(|r| {
            r.bridge.map(|b| BridgeInfo {
                provider: b.provider.as_str().to_owned(),
                capturer_user_id: b.lease.user_id.map(UserId::from),
                capturer_stale: b.lease.is_stale(now, &self.bridge),
                audio_mode: b.audio_mode.as_str().to_owned(),
            })
        }))
    }

    /// End the room as `actor` (host, or workspace owner/admin); idempotent when already ended.
    pub async fn end_room(&self, room: RoomId, actor: UserId) -> AppResult<()> {
        self.service
            .lifecycle(room, actor, domain::RoomAction::End)
            .await
            .map(|_| ())
    }

    /// Ids of the workspace's rooms that are `IN_PROGRESS` (at most 1000).
    pub async fn in_progress_rooms(&self, ws: WorkspaceId) -> AppResult<Vec<RoomId>> {
        let mut out = Vec::new();
        for page in 1..=10 {
            let req = warptalk_kernel::PageRequest {
                page,
                page_size: 100,
            };
            let got = self
                .repo
                .list(ws, Some(domain::RoomStatus::InProgress), &req)
                .await?;
            let n = got.items.len();
            out.extend(got.items.into_iter().map(|r| r.id));
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    /// Record a route's runtime status reported by the pipeline (`ROUTE_DEGRADED`): the
    /// `/audio-routes` snapshot then shows it. `false` when the route is not a current,
    /// unfinished route of `room`.
    pub async fn set_route_status(
        &self,
        room: RoomId,
        route: uuid::Uuid,
        status: warptalk_contracts::events::RouteStatus,
    ) -> AppResult<bool> {
        self.repo.set_route_status(room, route, status).await
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

/// Read-only bridge facts (`provider`: `google_meet|zoom|teams`; `audio_mode`: `voice|text`).
#[derive(Debug, Clone)]
pub struct BridgeInfo {
    pub provider: String,
    pub capturer_user_id: Option<UserId>,
    pub capturer_stale: bool,
    pub audio_mode: String,
}

/// Read-only participant facts (`status` is e.g. `WAITING`, `CONNECTED`).
#[derive(Debug, Clone)]
pub struct ParticipantSummary {
    pub status: String,
    pub display_name: String,
}
