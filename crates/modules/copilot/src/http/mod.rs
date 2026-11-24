//! HTTP surface of the copilot module.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{get, post};
use warptalk_web::JwtKeys;

use crate::application::service::CopilotService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<CopilotService>,
    pub jwt: JwtKeys,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/copilot/sessions",
            post(handlers::create).get(handlers::list),
        )
        .route("/copilot/sessions/{id}", get(handlers::get_one))
        .route("/copilot/sessions/{id}/end", post(handlers::end))
        .route("/copilot/sessions/{id}/segments", post(handlers::ingest))
        .route(
            "/copilot/sessions/{id}/stt-token",
            post(handlers::stt_token),
        )
        .with_state(state)
}
