use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use warptalk_kernel::{AppError, AppResult, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::{ApiResult, AuthUser};

use super::AppState;
use super::dto::{
    ChangeRoleRequest, CreateWorkspaceRequest, MemberDto, TransferRequest, WorkspaceDto,
    WorkspaceSearchQuery, map_page,
};
use crate::domain::WorkspaceSettings;

pub async fn create(
    State(s): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWorkspaceRequest>,
) -> AppResult<(StatusCode, Json<WorkspaceDto>)> {
    let ws = s
        .service
        .create(auth.user_id(), &req.name, req.description.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(ws.into())))
}

pub async fn list(
    State(s): State<AppState>,
    auth: AuthUser,
    Query(paging): Query<PageRequest>,
    Query(q): Query<WorkspaceSearchQuery>,
) -> ApiResult<Page<WorkspaceDto>> {
    let page = s
        .service
        .list(auth.user_id(), q.search.as_deref(), &paging)
        .await?;
    Ok(Json(map_page(page, WorkspaceDto::from)))
}

pub async fn get_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
) -> ApiResult<WorkspaceDto> {
    Ok(Json(s.service.get(id, auth.user_id()).await?.into()))
}

pub async fn delete_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
) -> AppResult<StatusCode> {
    s.service.delete(id, auth.user_id()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_settings(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
) -> ApiResult<WorkspaceSettings> {
    Ok(Json(s.service.get_settings(id, auth.user_id()).await?))
}

pub async fn update_settings(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
    Json(body): Json<WorkspaceSettings>,
) -> ApiResult<WorkspaceSettings> {
    Ok(Json(
        s.service.update_settings(id, auth.user_id(), body).await?,
    ))
}

pub async fn list_members(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
    Query(page_req): Query<PageRequest>,
) -> ApiResult<Page<MemberDto>> {
    let page = s
        .service
        .list_members(id, auth.user_id(), &page_req)
        .await?;
    Ok(Json(map_page(page, MemberDto::from)))
}

pub async fn change_role(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, user_id)): Path<(WorkspaceId, UserId)>,
    Json(req): Json<ChangeRoleRequest>,
) -> AppResult<StatusCode> {
    let role: WorkspaceRole = req
        .role
        .parse()
        .map_err(|_| AppError::validation("INVALID_ROLE", "role must be OWNER, ADMIN or MEMBER"))?;
    s.service
        .change_role(id, auth.user_id(), user_id, role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((id, user_id)): Path<(WorkspaceId, UserId)>,
) -> AppResult<StatusCode> {
    s.service.remove_member(id, auth.user_id(), user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn leave(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
) -> AppResult<StatusCode> {
    s.service.leave(id, auth.user_id()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn transfer(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(id): Path<WorkspaceId>,
    Json(req): Json<TransferRequest>,
) -> AppResult<StatusCode> {
    s.service
        .transfer_ownership(id, auth.user_id(), req.new_owner_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
