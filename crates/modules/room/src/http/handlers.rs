use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use warptalk_kernel::{AppError, AppResult, ParticipantId, RoomId};
use warptalk_web::{ApiResult, AuthUser};

use super::AppState;
use super::dto::{
    AudioModeRequest, BridgeStateResponse, ClaimBridgeRequest, ClaimBridgeResponse,
    CreateRoomRequest, HeartbeatRequest, JoinByCodeRequest, JoinByIdRequest, JoinedDto, ListQuery,
    PageDto, ParticipantDto, RoomDto, RoutesDto,
};
use crate::application::ports::RoomRecord;
use crate::application::service::{ClaimBridge, CreateRoom, JoinRoom};
use crate::domain::{AudioMode, RoomAction, RoomStatus};

pub async fn create(
    State(s): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateRoomRequest>,
) -> AppResult<(StatusCode, Json<RoomDto>)> {
    let room = s
        .service
        .create(
            auth.user_id(),
            CreateRoom {
                workspace_id: req.workspace_id,
                title: req.title,
                host_display_name: req.display_name,
                scheduled_at: req.scheduled_at,
                source_language: req.source_language,
                target_languages: req.target_languages,
                requires_approval: req.requires_approval,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(room.into())))
}

pub async fn list(
    State(s): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<PageDto<RoomDto>> {
    let status = match q.status.as_deref() {
        None | Some("") => None,
        Some(v) => Some(
            RoomStatus::parse(v)
                .ok_or_else(|| AppError::validation("INVALID_STATUS", "unknown room status"))?,
        ),
    };
    let page = s
        .service
        .list(auth.user_id(), q.workspace_id, status, &q.page_request())
        .await?;
    Ok(Json(PageDto::map(page, RoomDto::from)))
}

pub async fn get_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    Ok(Json(s.service.get(id, auth.user_id()).await?.into()))
}

pub async fn join_by_code(
    State(s): State<AppState>,
    auth: AuthUser,
    Json(req): Json<JoinByCodeRequest>,
) -> ApiResult<JoinedDto> {
    if req.room_code.trim().chars().count() != 12 {
        return Err(AppError::validation(
            "INVALID_ROOM_CODE",
            "Translation room code must be exactly 12 characters.",
        ));
    }
    let p = s
        .service
        .join_by_code(
            &req.room_code,
            auth.user_id(),
            JoinRoom {
                display_name: req.display_name,
                speak_language: req.speak_language,
                listen_language: req.listen_language,
            },
        )
        .await?;
    Ok(Json(p.into()))
}

pub async fn join_by_id(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
    Json(req): Json<JoinByIdRequest>,
) -> ApiResult<JoinedDto> {
    let p = s
        .service
        .join_by_id(
            id,
            auth.user_id(),
            JoinRoom {
                display_name: req.display_name,
                speak_language: req.speak_language,
                listen_language: req.listen_language,
            },
        )
        .await?;
    Ok(Json(p.into()))
}

async fn lifecycle(s: AppState, auth: AuthUser, id: RoomId, a: RoomAction) -> ApiResult<RoomDto> {
    Ok(Json(
        s.service.lifecycle(id, auth.user_id(), a).await?.into(),
    ))
}

pub async fn start(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::Start).await
}

pub async fn pause(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::Pause).await
}

pub async fn resume(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::Resume).await
}

pub async fn stop_translation(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::StopTranslation).await
}

pub async fn end(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::End).await
}

pub async fn cancel(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoomDto> {
    lifecycle(s, auth, id, RoomAction::Cancel).await
}

pub async fn participants(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<Vec<ParticipantDto>> {
    let list = s.service.participants(id, auth.user_id()).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

pub async fn admit(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, pid)): Path<(RoomId, ParticipantId)>,
) -> ApiResult<ParticipantDto> {
    Ok(Json(s.service.admit(id, pid, auth.user_id()).await?.into()))
}

pub async fn kick(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, pid)): Path<(RoomId, ParticipantId)>,
) -> ApiResult<ParticipantDto> {
    Ok(Json(s.service.kick(id, pid, auth.user_id()).await?.into()))
}

pub async fn leave(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<ParticipantDto> {
    Ok(Json(s.service.leave(id, auth.user_id()).await?.into()))
}

pub async fn generate_routes(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoutesDto> {
    Ok(Json(
        s.service.generate_routes(id, auth.user_id()).await?.into(),
    ))
}

pub async fn routes(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<RoutesDto> {
    Ok(Json(s.service.routes(id, auth.user_id()).await?.into()))
}

fn bridge_state(s: &AppState, room: RoomRecord) -> AppResult<Json<BridgeStateResponse>> {
    BridgeStateResponse::new(room, s.service.bridge_config())
        .map(Json)
        .ok_or_else(|| AppError::internal("bridge room without bridge state"))
}

pub async fn bridge_claim(
    State(s): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ClaimBridgeRequest>,
) -> AppResult<(StatusCode, Json<ClaimBridgeResponse>)> {
    let out = s
        .service
        .claim_bridge(
            auth.user_id(),
            ClaimBridge {
                workspace_id: req.workspace_id,
                provider: req.provider,
                meeting_url: req.meeting_url,
                code: req.code,
                title: req.title,
                display_name: req.display_name,
                source_language: req.source_language,
                target_languages: req.target_languages,
            },
        )
        .await?;
    let status = if out.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(ClaimBridgeResponse::new(out, s.service.bridge_config())),
    ))
}

pub async fn bridge_heartbeat(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
    body: Option<Json<HeartbeatRequest>>,
) -> AppResult<Json<BridgeStateResponse>> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let room = s
        .service
        .bridge_heartbeat(
            id,
            auth.user_id(),
            b.audio_flowing.unwrap_or(true),
            b.last_audio_at,
        )
        .await?;
    bridge_state(&s, room)
}

pub async fn bridge_takeover(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> AppResult<Json<BridgeStateResponse>> {
    let room = s.service.bridge_takeover(id, auth.user_id()).await?;
    bridge_state(&s, room)
}

pub async fn bridge_audio_mode(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
    Json(req): Json<AudioModeRequest>,
) -> AppResult<Json<BridgeStateResponse>> {
    let mode = AudioMode::parse(req.mode.trim())
        .ok_or_else(|| AppError::validation("INVALID_AUDIO_MODE", "mode must be voice or text"))?;
    let room = s
        .service
        .set_bridge_audio_mode(id, auth.user_id(), mode)
        .await?;
    bridge_state(&s, room)
}
