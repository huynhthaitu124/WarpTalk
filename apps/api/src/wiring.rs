//! Module construction and cross-module adapters.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::get;
use sqlx::PgPool;
use warptalk_config::JwtConfig;
use warptalk_identity::{
    IdentityDeps, IdentityModule, IdentityQueries, Policy, VerificationMailer,
};
use warptalk_kernel::{AppResult, Clock, SystemClock, UserId};
use warptalk_web::JwtKeys;
use warptalk_workspace::application::ports::UserDirectory;
use warptalk_workspace::{UserSummary, WorkspaceDeps, WorkspaceModule};

/// Adapter: workspace's `UserDirectory` port backed by identity's read facade.
pub struct IdentityUserDirectory(pub IdentityQueries);

#[async_trait]
impl UserDirectory for IdentityUserDirectory {
    async fn summaries(&self, ids: &[UserId]) -> AppResult<HashMap<UserId, UserSummary>> {
        let mut out = HashMap::with_capacity(ids.len());
        // TODO batch query
        for id in ids {
            if let Some(u) = self.0.user_summary(*id).await? {
                out.insert(
                    *id,
                    UserSummary {
                        email: u.email,
                        full_name: u.full_name,
                        avatar_url: u.avatar_url,
                    },
                );
            }
        }
        Ok(out)
    }
}

/// Build the full API router (`/health` plus modules under `/api/v1`).
pub fn build(
    pool: PgPool,
    jwt: JwtKeys,
    jwt_cfg: &JwtConfig,
    cors_origins: &[String],
    mailer: Arc<dyn VerificationMailer>,
) -> Router {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let refresh_secs = i64::try_from(jwt_cfg.refresh_ttl_seconds).unwrap_or(i64::from(i32::MAX));
    let policy = Policy {
        refresh_ttl: chrono::Duration::seconds(refresh_secs),
        ..Policy::default()
    };

    let identity = IdentityModule::new(IdentityDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: clock.clone(),
        mailer,
        policy,
    });
    let workspace = WorkspaceModule::new(WorkspaceDeps {
        pool,
        jwt,
        clock,
        users: Arc::new(IdentityUserDirectory(identity.queries())),
    });

    let api = Router::new()
        .merge(identity.router())
        .merge(workspace.router());
    let router = Router::new()
        .route("/health", get(warptalk_web::health))
        .nest("/api/v1", api);
    warptalk_web::common_layers(router, cors_origins)
}
