//! `GET /features?workspaceId=`: the effective product feature flags (ADR 0001).
//!
//! Flags live in the `features` settings namespace (`Platform < Workspace`; admins edit them with
//! `PUT /settings/features?scope=platform|workspace`, snake_case keys). This route answers
//! camelCase booleans:
//!
//! - no `Authorization` header: the platform flags (workspace id ignored; the login screen needs
//!   them before there is a user);
//! - bearer token, no `workspaceId`: the platform flags;
//! - bearer token + `workspaceId`: the workspace's effective flags, members only (403 otherwise).
//!
//! The flags are advisory: clients hide the UI of a disabled feature. The API does not refuse the
//! translation routes of existing native rooms when `translationMeetings` is off (so no existing
//! room or integration breaks); the pipeline/LiveKit stack is simply not deployed by default.

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};
use warptalk_settings::{FeatureFlags, NS_FEATURES, ScopeChain, SettingsStore};
use warptalk_web::extract::Query;
use warptalk_web::{ApiResult, JwtKeys};
use warptalk_workspace::WorkspaceQueries;

#[derive(Clone)]
pub struct FeaturesState {
    pub store: SettingsStore,
    pub jwt: JwtKeys,
    pub workspaces: WorkspaceQueries,
}

impl FromRef<FeaturesState> for JwtKeys {
    fn from_ref(s: &FeaturesState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: FeaturesState) -> Router {
    Router::new()
        .route("/features", get(get_features))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeaturesQuery {
    workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeaturesDto {
    pub translation_meetings: bool,
    pub dubbing: bool,
    pub voice_clone: bool,
    pub external_bridge: bool,
    pub copilot: bool,
}

impl From<FeatureFlags> for FeaturesDto {
    fn from(f: FeatureFlags) -> Self {
        Self {
            translation_meetings: f.translation_meetings,
            dubbing: f.dubbing,
            voice_clone: f.voice_clone,
            external_bridge: f.external_bridge,
            copilot: f.copilot,
        }
    }
}

/// `Ok(None)`: no credentials. A present but invalid bearer token is a 401, not "anonymous".
fn optional_user(headers: &HeaderMap, jwt: &JwtKeys) -> AppResult<Option<UserId>> {
    let Some(raw) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let token = raw
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .and_then(|(scheme, tok)| scheme.eq_ignore_ascii_case("bearer").then(|| tok.trim()))
        .ok_or_else(|| AppError::unauthorized("malformed authorization header"))?;
    Ok(Some(jwt.verify(token)?.sub))
}

async fn get_features(
    State(st): State<FeaturesState>,
    headers: HeaderMap,
    Query(q): Query<FeaturesQuery>,
) -> ApiResult<FeaturesDto> {
    let user = optional_user(&headers, &st.jwt)?;
    let chain = match (user, q.workspace_id) {
        (Some(user), Some(ws)) => {
            let ws = WorkspaceId::from_uuid(ws);
            if st.workspaces.member_role(ws, user).await?.is_none() {
                return Err(AppError::forbidden("not a member of this workspace"));
            }
            ScopeChain::workspace(ws)
        }
        _ => ScopeChain::platform(),
    };
    let flags = st.store.get::<FeatureFlags>(NS_FEATURES, chain).await?;
    Ok(axum::Json(FeaturesDto::from(flags)))
}
