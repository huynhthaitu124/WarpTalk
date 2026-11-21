use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, RoomId};
use warptalk_web::AuthUser;
use warptalk_web::extract::{Json, Path, Query};

use super::AppState;

type ApiResult<T> = AppResult<Json<T>>;
use super::dto::{
    ChatHistoryQuery, ChatMessageDto, HideChatRequest, JoinRequest, JoinResponse, PostChatRequest,
};

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

/// `201` for a new message, `200` when `clientMessageId` was already stored (idempotent replay).
pub async fn post_chat(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
    Json(req): Json<PostChatRequest>,
) -> AppResult<(StatusCode, Json<ChatMessageDto>)> {
    let (msg, created) = s
        .service
        .post_chat(auth.user_id(), room, &req.content, &req.client_message_id)
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(msg.into())))
}

pub async fn chat_history(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
    Query(q): Query<ChatHistoryQuery>,
) -> ApiResult<Vec<ChatMessageDto>> {
    let msgs = s
        .service
        .chat_history(auth.user_id(), room, q.before, q.limit)
        .await?;
    Ok(Json(msgs.into_iter().map(Into::into).collect()))
}

pub async fn hide_chat(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((room, message)): Path<(RoomId, Uuid)>,
    body: Option<Json<HideChatRequest>>,
) -> ApiResult<ChatMessageDto> {
    let reason = body.and_then(|Json(b)| b.reason);
    Ok(Json(
        s.service
            .hide_chat(auth.user_id(), room, message, reason.as_deref())
            .await?
            .into(),
    ))
}
