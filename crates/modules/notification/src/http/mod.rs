//! HTTP surface: `GET|PUT notifications/preferences`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::AppResult;
use warptalk_web::extract::Json;
use warptalk_web::{AuthUser, JwtKeys};

use crate::application::PreferenceService;
use crate::domain::{NotificationPreference, PreferenceUpdate};

#[derive(Clone)]
pub(crate) struct HttpState {
    pub preferences: Arc<PreferenceService>,
    pub jwt: JwtKeys,
}

impl FromRef<HttpState> for JwtKeys {
    fn from_ref(s: &HttpState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub(crate) fn router(state: HttpState) -> Router {
    Router::new()
        .route(
            "/notifications/preferences",
            get(get_preferences).put(put_preferences),
        )
        .with_state(state)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreferenceDto {
    id: Uuid,
    user_id: Uuid,
    notification_type: String,
    email_enabled: bool,
    push_enabled: bool,
    in_app_enabled: bool,
    updated_at: DateTime<Utc>,
}

impl From<NotificationPreference> for PreferenceDto {
    fn from(p: NotificationPreference) -> Self {
        Self {
            id: p.id,
            user_id: p.user_id,
            notification_type: p.notification_type,
            email_enabled: p.email_enabled,
            push_enabled: p.push_enabled,
            in_app_enabled: p.in_app_enabled,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    email_enabled: Option<bool>,
    push_enabled: Option<bool>,
    in_app_enabled: Option<bool>,
}

async fn get_preferences(
    State(s): State<HttpState>,
    user: AuthUser,
) -> AppResult<Json<PreferenceDto>> {
    let p = s.preferences.get(user.user_id().as_uuid()).await?;
    Ok(Json(p.into()))
}

async fn put_preferences(
    State(s): State<HttpState>,
    user: AuthUser,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<PreferenceDto>> {
    let p = s
        .preferences
        .update(
            user.user_id().as_uuid(),
            PreferenceUpdate {
                email_enabled: body.email_enabled,
                push_enabled: body.push_enabled,
                in_app_enabled: body.in_app_enabled,
            },
        )
        .await?;
    Ok(Json(p.into()))
}
