//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{get, post, put};
use warptalk_web::JwtKeys;

use crate::application::service::RoomService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<RoomService>,
    pub jwt: JwtKeys,
}

impl AppState {
    fn room_dto(&self, r: crate::application::ports::RoomRecord) -> dto::RoomDto {
        dto::RoomDto::new(r, &self.service.bridge_config(), self.service.now())
    }
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/translation-rooms",
            post(handlers::create).get(handlers::list),
        )
        .route("/translation-rooms/join", post(handlers::join_by_code))
        .route(
            "/translation-rooms/bridge/claim",
            post(handlers::bridge_claim),
        )
        .route(
            "/translation-rooms/{id}/bridge/capturer/heartbeat",
            post(handlers::bridge_heartbeat),
        )
        .route(
            "/translation-rooms/{id}/bridge/capturer/takeover",
            post(handlers::bridge_takeover),
        )
        .route(
            "/translation-rooms/{id}/bridge/capturer/release",
            post(handlers::bridge_release),
        )
        .route(
            "/translation-rooms/{id}/bridge/audio-mode",
            put(handlers::bridge_audio_mode),
        )
        .route("/translation-rooms/{id}", get(handlers::get_one))
        .route("/translation-rooms/{id}/join", post(handlers::join_by_id))
        .route("/translation-rooms/{id}/start", post(handlers::start))
        .route("/translation-rooms/{id}/pause", post(handlers::pause))
        .route("/translation-rooms/{id}/resume", post(handlers::resume))
        .route(
            "/translation-rooms/{id}/translation/stop",
            post(handlers::stop_translation),
        )
        .route("/translation-rooms/{id}/end", post(handlers::end))
        .route("/translation-rooms/{id}/cancel", post(handlers::cancel))
        .route(
            "/translation-rooms/{id}/participants",
            get(handlers::participants),
        )
        .route(
            "/translation-rooms/{id}/participants/me/leave",
            put(handlers::leave),
        )
        .route(
            "/translation-rooms/{id}/participants/{participant_id}/admit",
            put(handlers::admit),
        )
        .route(
            "/translation-rooms/{id}/participants/{participant_id}/kick",
            put(handlers::kick),
        )
        .route(
            "/translation-rooms/{id}/audio-routes/generate",
            post(handlers::generate_routes),
        )
        .route(
            "/translation-rooms/{id}/audio-routes",
            get(handlers::routes),
        )
        .with_state(state)
}
