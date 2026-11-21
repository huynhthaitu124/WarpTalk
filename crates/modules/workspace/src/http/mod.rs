//! HTTP surface: axum routes, handlers, DTOs.

mod dto;
mod handlers;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{delete, get, post, put};
use warptalk_web::JwtKeys;

use crate::application::invitations::InvitationService;
use crate::application::service::WorkspaceService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<WorkspaceService>,
    pub invitations: Arc<InvitationService>,
    pub jwt: JwtKeys,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/workspaces", post(handlers::create).get(handlers::list))
        .route(
            "/workspaces/{id}",
            get(handlers::get_one).delete(handlers::delete_one),
        )
        .route(
            "/workspaces/{id}/settings",
            get(handlers::get_settings).put(handlers::update_settings),
        )
        .route("/workspaces/{id}/members", get(handlers::list_members))
        .route(
            "/workspaces/{id}/members/{user_id}/role",
            put(handlers::change_role),
        )
        .route(
            "/workspaces/{id}/members/{user_id}",
            delete(handlers::remove_member),
        )
        .route("/workspaces/{id}/leave", post(handlers::leave))
        .route(
            "/workspaces/{id}/transfer-ownership",
            post(handlers::transfer),
        )
        .route(
            "/workspaces/{id}/invitations",
            post(handlers::invite).get(handlers::list_invitations),
        )
        .route(
            "/workspaces/{id}/invitations/{inv_id}",
            delete(handlers::revoke_invitation),
        )
        .route(
            "/workspaces/{id}/invitations/{inv_id}/resend",
            post(handlers::resend_invitation),
        )
        .route("/invitations/accept", post(handlers::accept_invitation))
        .route("/invitations/pending", get(handlers::pending_invitations))
        .route(
            "/invitations/{inv_id}/accept",
            post(handlers::accept_invitation_by_id),
        )
        .with_state(state)
}
