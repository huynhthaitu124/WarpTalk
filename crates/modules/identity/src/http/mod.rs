//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use warptalk_web::JwtKeys;

use crate::application::{AuthService, ProfileService};
use crate::domain::IdentityError;

#[derive(Clone)]
pub(crate) struct HttpState {
    pub auth: Arc<AuthService>,
    pub profile: Arc<ProfileService>,
    pub jwt: JwtKeys,
}

impl FromRef<HttpState> for JwtKeys {
    fn from_ref(s: &HttpState) -> JwtKeys {
        s.jwt.clone()
    }
}

impl IntoResponse for IdentityError {
    fn into_response(self) -> Response {
        match self {
            Self::App(e) => e.into_response(),
            Self::Coded {
                status,
                code,
                message,
            } => {
                let status = axum::http::StatusCode::from_u16(status)
                    .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                (
                    status,
                    axum::Json(serde_json::json!({ "code": code, "message": message })),
                )
                    .into_response()
            }
        }
    }
}

pub(crate) fn router(state: HttpState) -> Router {
    Router::new()
        .route("/auth/register", post(handlers::register))
        .route("/auth/verify-email", post(handlers::verify_email))
        .route("/auth/login", post(handlers::login))
        .route("/auth/refresh", post(handlers::refresh))
        .route("/auth/logout", post(handlers::logout))
        .route(
            "/me/profile",
            get(handlers::get_profile).put(handlers::put_profile),
        )
        .route(
            "/me/settings",
            get(handlers::get_settings).put(handlers::put_settings),
        )
        .with_state(state)
}
