//! HTTP surface: axum routes, handlers, DTOs.

pub mod chat;
mod dto;
mod handlers;
pub mod summary;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{post, put};
use warptalk_web::JwtKeys;

use crate::application::LlmConfigService;

#[derive(Clone)]
pub struct AppState {
    pub llm_config: Arc<LlmConfigService>,
    pub jwt: JwtKeys,
    /// Post-session summary routes, mounted when configured.
    pub summary: Option<summary::SummaryApi>,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: AppState) -> Router {
    let summary = state
        .summary
        .clone()
        .map(|api| summary::router(api, state.jwt.clone()));
    let router = Router::new()
        .route(
            "/ai/workspaces/{id}/config",
            axum::routing::get(handlers::get_config).put(handlers::put_config),
        )
        .route(
            "/ai/workspaces/{id}/credentials/{provider}",
            put(handlers::put_credential).delete(handlers::delete_credential),
        )
        .route(
            "/ai/workspaces/{id}/credentials/{provider}/test",
            post(handlers::test_credential),
        )
        .with_state(state);
    match summary {
        Some(r) => router.merge(r),
        None => router,
    }
}
