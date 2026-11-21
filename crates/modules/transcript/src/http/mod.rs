//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
pub mod glossary;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{get, post};
use warptalk_web::JwtKeys;

use crate::application::service::TranscriptService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<TranscriptService>,
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
        .route("/transcripts/{room_id}", get(handlers::get_by_room))
        .route(
            "/transcripts/{id}/corrections",
            post(handlers::correct).get(handlers::list_corrections),
        )
        .route("/transcripts/{id}/finalize", post(handlers::finalize))
        .with_state(state)
}
