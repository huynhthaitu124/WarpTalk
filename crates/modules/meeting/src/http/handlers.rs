use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use warptalk_kernel::{AppError, AppResult, RoomId};
use warptalk_web::AuthUser;
use warptalk_web::extract::{Json, Path};

use super::AppState;

type ApiResult<T> = AppResult<Json<T>>;
use super::dto::{JoinRequest, JoinResponse};

pub async fn join(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
    body: Option<Json<JoinRequest>>,
) -> ApiResult<JoinResponse> {
    let name = body.and_then(|Json(b)| b.display_name);
    let grant = s.service.join(auth.user_id(), room, name).await?;
    Ok(Json(grant.into()))
}

pub async fn bridge_token(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
) -> ApiResult<JoinResponse> {
    Ok(Json(
        s.service.bridge_token(auth.user_id(), room).await?.into(),
    ))
}

pub async fn end(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
) -> AppResult<StatusCode> {
    s.service.end(auth.user_id(), room).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// No `AuthUser`: LiveKit authenticates with a JWT over the body hash instead.
pub async fn livekit_webhook(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<StatusCode> {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::unauthorized("missing webhook authorization"))?;
    s.service.handle_webhook(&body, auth).await?;
    Ok(StatusCode::OK)
}
