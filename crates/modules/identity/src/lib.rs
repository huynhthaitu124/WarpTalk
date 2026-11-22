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

pub use application::voice::{
    NoopVoiceCloner, NoopVoiceProviderAdmin, NoopVoiceStatePublisher, VoiceCloner, VoiceDeps,
    VoiceProviderAdmin, VoiceRuntimeState, VoiceStatePublisher,
};
pub use application::{IdentityRepository, VerificationMailer};
pub use domain::{Policy, UserSummary};
pub use infrastructure::{
    CartesiaVoice, DevMailCapture, LogMailer, RedisVoiceStatePublisher, consent_key,
};

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
        Migration {
            version: 3,
            name: "voice",
            sql: include_str!("../migrations/0003_voice.sql"),
        },
    ],
};

pub struct IdentityDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub mailer: Arc<dyn VerificationMailer>,
    pub policy: Policy,
    /// Voice provider / runtime ports (no-ops by default).
    pub voice: VoiceDeps,
}

pub struct IdentityModule {
    state: http::HttpState,
    repo: Arc<dyn IdentityRepository>,
    voice: Arc<application::voice::VoiceService>,
}

impl IdentityModule {
    pub fn new(deps: IdentityDeps) -> Self {
        let repo: Arc<dyn IdentityRepository> =
            Arc::new(infrastructure::PgIdentityRepository::new(deps.pool.clone()));
        let voice = Arc::new(application::voice::VoiceService::new(
            Arc::new(infrastructure::PgVoiceRepository::new(deps.pool)),
            deps.voice,
            deps.clock.clone(),
        ));
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
                voice: voice.clone(),
                jwt: deps.jwt,
            },
            repo,
            voice,
        }
    }

    pub fn router(&self) -> Router {
        http::router(self.state.clone())
    }

    /// Read-only facade for other modules (wired by `apps/api`).
    pub fn queries(&self) -> IdentityQueries {
        IdentityQueries {
            repo: self.repo.clone(),
            voice: self.voice.clone(),
        }
    }

    /// Retries provider voice deletions that failed earlier (call from a periodic job).
    pub async fn retry_voice_deletions(&self, limit: i64) -> AppResult<usize> {
        self.voice.retry_pending_deletions(limit).await
    }
}

#[derive(Clone)]
pub struct IdentityQueries {
    repo: Arc<dyn IdentityRepository>,
    voice: Arc<application::voice::VoiceService>,
}

impl IdentityQueries {
    pub async fn user_summary(&self, id: UserId) -> AppResult<Option<UserSummary>> {
        self.repo.find_summary_by_id(id).await
    }

    pub async fn find_by_email(&self, email: &str) -> AppResult<Option<UserSummary>> {
        self.repo.find_summary_by_email(email).await
    }
}

impl IdentityQueries {
    /// Active voice-clone consent (BR-94).
    pub async fn voice_clone_consent(&self, user: UserId) -> AppResult<bool> {
        self.voice.voice_clone_consent(user).await
    }

    /// The user's provider voice id; `None` without active consent or an active profile.
    pub async fn provider_voice(&self, user: UserId) -> AppResult<Option<String>> {
        self.voice.provider_voice(user).await
    }
}
