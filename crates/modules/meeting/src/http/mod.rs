//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::post;
use warptalk_web::JwtKeys;

use crate::application::service::MeetingService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<MeetingService>,
    pub jwt: JwtKeys,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`. The webhook route is unauthenticated (signature-verified).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/meetings/webhooks/livekit",
            post(handlers::livekit_webhook),
        )
        .route("/meetings/{room_id}/join", post(handlers::join))
        .route(
            "/meetings/{room_id}/bridge-token",
            post(handlers::bridge_token),
        )
        .route("/meetings/{room_id}/end", post(handlers::end))
        .route(
            "/meetings/{room_id}/chat/messages",
            post(handlers::post_chat).get(handlers::chat_history),
        )
        .route(
            "/meetings/{room_id}/chat/messages/{message_id}/hide",
            post(handlers::hide_chat),
        )
        .with_state(state)
}
