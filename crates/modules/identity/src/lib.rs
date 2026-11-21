//! Accounts, sessions, JWT + rotating refresh tokens, profile and settings (schema `auth`).
//!
//! Layers (dependencies point inward):
//! - `domain`: value objects, policy, password hashing, validation. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, log mailer).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::{AppResult, Clock, UserId};
use warptalk_web::JwtKeys;

pub use application::{IdentityRepository, VerificationMailer};
pub use domain::{Policy, UserSummary};
pub use infrastructure::{DevMailCapture, LogMailer};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "identity",
    migrations: &[
        Migration {
            version: 1,
            name: "init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "auth_flow",
            sql: include_str!("../migrations/0002_auth_flow.sql"),
        },
    ],
};

pub struct IdentityDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub mailer: Arc<dyn VerificationMailer>,
    pub policy: Policy,
}

pub struct IdentityModule {
    state: http::HttpState,
    repo: Arc<dyn IdentityRepository>,
}

impl IdentityModule {
    pub fn new(deps: IdentityDeps) -> Self {
        let repo: Arc<dyn IdentityRepository> =
            Arc::new(infrastructure::PgIdentityRepository::new(deps.pool));
        let auth = Arc::new(application::AuthService::new(
            repo.clone(),
            deps.mailer,
            deps.jwt.clone(),
            deps.clock.clone(),
            deps.policy,
        ));
        let profile = Arc::new(application::ProfileService::new(repo.clone(), deps.clock));
        Self {
            state: http::HttpState {
                auth,
                profile,
                jwt: deps.jwt,
            },
            repo,
        }
    }

    pub fn router(&self) -> Router {
        http::router(self.state.clone())
    }

    /// Read-only facade for other modules (wired by `apps/api`).
    pub fn queries(&self) -> IdentityQueries {
        IdentityQueries {
            repo: self.repo.clone(),
        }
    }
}

#[derive(Clone)]
pub struct IdentityQueries {
    repo: Arc<dyn IdentityRepository>,
}

impl IdentityQueries {
    pub async fn user_summary(&self, id: UserId) -> AppResult<Option<UserSummary>> {
        self.repo.find_summary_by_id(id).await
    }

    pub async fn find_by_email(&self, email: &str) -> AppResult<Option<UserSummary>> {
        self.repo.find_summary_by_email(email).await
    }
}
