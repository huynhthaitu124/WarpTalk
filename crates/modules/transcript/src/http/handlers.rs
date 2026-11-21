use axum::extract::State;
use axum::http::StatusCode;
use warptalk_kernel::{AppResult, RoomId, TranscriptId};
use warptalk_web::AuthUser;
use warptalk_web::extract::{Json, Path, Query};

use super::AppState;
use super::dto::{CorrectionDto, CorrectionRequest, CorrectionsQuery, LangQuery, TranscriptDto};

type ApiResult<T> = AppResult<Json<T>>;

pub async fn get_by_room(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
    Query(q): Query<LangQuery>,
) -> ApiResult<TranscriptDto> {
    let detail = s
        .service
        .get_for_room(auth.user_id(), room, q.lang.as_deref())
        .await?;
    Ok(Json(detail.into()))
}

pub async fn correct(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<TranscriptId>,
    Json(body): Json<CorrectionRequest>,
) -> AppResult<(StatusCode, Json<CorrectionDto>)> {
    let c = s
        .service
        .correct(
            auth.user_id(),
            id,
            body.segment_id,
            &body.text,
            body.reason.as_deref(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(c.into())))
}

pub async fn list_corrections(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<TranscriptId>,
    Query(q): Query<CorrectionsQuery>,
) -> ApiResult<Vec<CorrectionDto>> {
    let list = s
        .service
        .corrections(auth.user_id(), id, q.segment_id)
        .await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

pub async fn finalize(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<TranscriptId>,
) -> AppResult<StatusCode> {
    s.service.finalize(auth.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
