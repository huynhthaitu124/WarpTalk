use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use warptalk_kernel::{AppError, AppResult, Page, PageRequest, RoomId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{ApiResult, AuthUser};

use super::AppState;
use super::dto::{
    AudioTicketResponse, CreateSessionRequest, IngestRequest, IngestResponse, ListQuery,
    SessionDto, SttTokenBody, SttTokenResponse,
};
use crate::domain::MAX_SEGMENTS_PER_CALL;

pub async fn create(
    State(s): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateSessionRequest>,
) -> AppResult<(StatusCode, Json<SessionDto>)> {
    let v = s.service.create(auth.user_id(), req.into()).await?;
    Ok((StatusCode::CREATED, Json(v.into())))
}

pub async fn list(
    State(s): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Page<SessionDto>> {
    let mut page = PageRequest::default();
    if let Some(p) = q.page {
        page.page = p;
    }
    if let Some(n) = q.page_size {
        page.page_size = n;
    }
    let p = s
        .service
        .list(auth.user_id(), q.workspace_id.into(), &page)
        .await?;
    Ok(axum::Json(Page {
        items: p.items.into_iter().map(Into::into).collect(),
        page: p.page,
        page_size: p.page_size,
        total: p.total,
    }))
}

pub async fn get_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<SessionDto> {
    Ok(axum::Json(s.service.get(id, auth.user_id()).await?.into()))
}

pub async fn end(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> ApiResult<SessionDto> {
    Ok(axum::Json(s.service.end(id, auth.user_id()).await?.into()))
}

pub async fn ingest(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
    Json(req): Json<IngestRequest>,
) -> ApiResult<IngestResponse> {
    if req.segments.len() > MAX_SEGMENTS_PER_CALL {
        return Err(AppError::validation(
            "INVALID_SEGMENTS",
            format!("at most {MAX_SEGMENTS_PER_CALL} segments per call"),
        ));
    }
    let mut segments = Vec::with_capacity(req.segments.len());
    for (i, b) in req.segments.into_iter().enumerate() {
        segments.push(b.into_segment().ok_or_else(|| {
            AppError::validation(
                "INVALID_SEGMENT",
                format!("segments[{i}]: channel must be `me` or `others`"),
            )
        })?);
    }
    let out = s.service.ingest(id, auth.user_id(), segments).await?;
    Ok(axum::Json(out.into()))
}

/// The response carries a secret: never cacheable. The body is optional JSON
/// (`{ "languageMode", "languages", "vocabulary" }`); empty means the session defaults.
pub async fn stt_token(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
    body: Bytes,
) -> AppResult<Response> {
    let prefs = if body.iter().all(u8::is_ascii_whitespace) {
        SttTokenBody::default()
    } else {
        serde_json::from_slice::<SttTokenBody>(&body).map_err(|e| {
            AppError::validation("INVALID_BODY", format!("invalid stt-token body: {e}"))
        })?
    }
    .into_prefs()?;
    let t = s.service.stt_token(id, auth.user_id(), prefs).await?;
    let mut resp = axum::Json(SttTokenResponse::from(t)).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// One-time ticket for `GET /copilot/sessions/{id}/audio?token=...` (never cacheable).
pub async fn audio_ticket(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<RoomId>,
) -> AppResult<Response> {
    let t = s.service.issue_audio_ticket(id, auth.user_id()).await?;
    let mut resp = axum::Json(AudioTicketResponse::from(t)).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}
