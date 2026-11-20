//! Tenancy, members, roles, invitations, verified domains, documents, policies, entitlements, admin audit (schema workspace).
//!
//! Slice 1: workspaces, settings and members. Layers (dependencies point inward):
//! - `domain`: value objects and authorization policy. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::{AppResult, Clock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;

use application::ports::{UserDirectory, WorkspaceRepo};
use application::service::WorkspaceService;
use infrastructure::pg_repo::PgWorkspaceRepo;

pub use application::ports::{NoopUserDirectory, UserSummary};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "workspace",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct WorkspaceDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub users: Arc<dyn UserDirectory>,
}

pub struct WorkspaceModule {
    service: Arc<WorkspaceService>,
    repo: Arc<dyn WorkspaceRepo>,
    jwt: JwtKeys,
}

impl WorkspaceModule {
    pub fn new(deps: WorkspaceDeps) -> Self {
        let repo: Arc<dyn WorkspaceRepo> = Arc::new(PgWorkspaceRepo::new(deps.pool));
        let service = Arc::new(WorkspaceService::new(repo.clone(), deps.users, deps.clock));
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
    pub fn queries(&self) -> WorkspaceQueries {
        WorkspaceQueries {
            repo: self.repo.clone(),
        }
    }
}

/// Public query facade for other modules (membership and workspace availability).
#[derive(Clone)]
pub struct WorkspaceQueries {
    repo: Arc<dyn WorkspaceRepo>,
}

impl WorkspaceQueries {
    /// Role of `user` in `ws` if an active member of an active workspace.
    pub async fn member_role(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<Option<WorkspaceRole>> {
        self.repo.member_role(ws, user).await
    }

    /// True when the workspace exists and is neither soft-deleted nor deactivated (BR-43).
    pub async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool> {
        self.repo.is_active(ws).await
    }
}
