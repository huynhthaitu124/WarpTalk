//! HTTP surface (paths relative to `/api/v1`).

mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{get, post, put};
use warptalk_web::JwtKeys;

use crate::application::service::BillingService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<BillingService>,
    pub jwt: JwtKeys,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/billing/plans", get(handlers::plans))
        .route("/workspaces/{id}/billing", get(handlers::overview))
        .route("/workspaces/{id}/billing/checkout", post(handlers::checkout))
        .route("/workspaces/{id}/billing/portal", post(handlers::portal))
        .route("/workspaces/{id}/billing/usage", get(handlers::usage))
        .route(
            "/workspaces/{id}/billing/transactions",
            get(handlers::transactions),
        )
        // No auth: authenticated by the Stripe signature over the raw body.
        .route("/billing/webhooks/stripe", post(handlers::stripe_webhook))
        .route("/admin/billing/rate-card", put(handlers::set_rate_card))
        .route(
            "/admin/workspaces/{id}/credits/adjust",
            post(handlers::adjust_credits),
        )
        .with_state(state)
}
