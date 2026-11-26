//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::FromRef;
use axum::routing::{delete, get, post};
use warptalk_web::JwtKeys;

use crate::application::voice::VoiceService;
use crate::application::{AuthService, ProfileService};

#[derive(Clone)]
pub(crate) struct HttpState {
    pub auth: Arc<AuthService>,
    pub profile: Arc<ProfileService>,
    pub voice: Arc<VoiceService>,
    pub jwt: JwtKeys,
}

impl FromRef<HttpState> for JwtKeys {
    fn from_ref(s: &HttpState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub(crate) fn router(state: HttpState) -> Router {
    Router::new()
        .route("/auth/register", post(handlers::register))
        .route("/auth/verify-email", post(handlers::verify_email))
        .route(
            "/auth/resend-verification",
            post(handlers::resend_verification),
        )
        .route("/auth/forgot-password", post(handlers::forgot_password))
        .route("/auth/reset-password", post(handlers::reset_password))
        .route("/auth/google", post(handlers::google_login))
        .route("/auth/login", post(handlers::login))
        .route("/auth/refresh", post(handlers::refresh))
        .route("/auth/logout", post(handlers::logout))
        .route("/auth/sessions", get(handlers::list_sessions))
        .route(
            "/auth/sessions/revoke-others",
            post(handlers::revoke_other_sessions),
        )
        .route("/auth/sessions/{id}", delete(handlers::revoke_session))
        .route("/me", get(handlers::me))
        .route("/me/password", post(handlers::change_password))
        .route(
            "/me/google/link",
            post(handlers::link_google).delete(handlers::unlink_google),
        )
        .route(
            "/me/profile",
            get(handlers::get_profile).put(handlers::put_profile),
        )
        .route(
            "/me/settings",
            get(handlers::get_settings).put(handlers::put_settings),
        )
        .route("/me/voice", get(handlers::get_voice))
        .route(
            "/me/voice/consent",
            post(handlers::grant_voice_consent).delete(handlers::revoke_voice_consent),
        )
        .route(
            "/me/voice/enroll",
            // 30 s of 48 kHz stereo 16-bit is ~5.8 MB (7.7 MB base64).
            post(handlers::enroll_voice).layer(DefaultBodyLimit::max(9 * 1024 * 1024)),
        )
        .with_state(state)
}
