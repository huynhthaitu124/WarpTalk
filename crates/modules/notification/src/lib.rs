//! In-app, email, push, templates, preferences, admin broadcast (schema notification).
//!
//! Slice 1: per-user notification preferences. Layers (dependencies point inward):
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
use warptalk_kernel::Clock;
use warptalk_web::JwtKeys;

pub use application::PreferenceService;
pub use domain::{NotificationPreference, PreferenceUpdate};

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "notification",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct NotificationDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
}

pub struct NotificationModule {
    preferences: Arc<PreferenceService>,
    jwt: JwtKeys,
}

impl NotificationModule {
    pub fn new(deps: NotificationDeps) -> Self {
        let repo = Arc::new(infrastructure::PgPreferenceRepository::new(deps.pool));
        Self {
            preferences: Arc::new(PreferenceService::new(repo, deps.clock)),
            jwt: deps.jwt,
        }
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::HttpState {
            preferences: self.preferences.clone(),
            jwt: self.jwt.clone(),
        })
    }
}
