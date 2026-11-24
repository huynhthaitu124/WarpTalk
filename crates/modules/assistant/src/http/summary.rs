//! REST of the post-session summary (paths relative to `/api/v1`, camelCase bodies).
//!
//! Access: a room's summary and action items are visible to its host, its participants and the
//! workspace's OWNER/ADMIN; regenerate needs the host or an OWNER/ADMIN. The workspace-wide
//! list shows OWNER/ADMIN everything and other members only the rooms they host or joined.

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};

use crate::summary::repo::{ItemFilter, ItemPatch};
use crate::summary::{
    ActionItem, RoomAccess, RoomAccessInfo, SummaryContent, SummaryJob, SummaryService,
    SummaryStatus,
};

/// Everything the summary routes need.
#[derive(Clone)]
pub struct SummaryApi {
    pub service: Arc<SummaryService>,
    pub access: Arc<dyn RoomAccess>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
struct State_ {
    api: SummaryApi,
    jwt: JwtKeys,
}

impl FromRef<State_> for JwtKeys {
    fn from_ref(s: &State_) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(api: SummaryApi, jwt: JwtKeys) -> Router {
    Router::new()
        .route("/meetings/{room_id}/summary", get(get_summary))
        .route("/meetings/{room_id}/summary/regenerate", post(regenerate))
        .route("/meetings/{room_id}/action-items", get(list_room_items))
        .route(
            "/meetings/{room_id}/action-items/{item_id}",
            patch(patch_item),
        )
        .route("/workspaces/{id}/action-items", get(list_workspace_items))
        .with_state(State_ { api, jwt })
}

// ------------------------------------------------------------------ DTOs

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryView {
    pub id: Uuid,
    pub room_id: Uuid,
    pub version: i32,
    pub status: SummaryStatus,
    pub trigger: String,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub content: Option<SummaryContent>,
    pub error_code: Option<String>,
    pub error: Option<String>,
    pub attempts: i32,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<SummaryJob> for SummaryView {
    fn from(j: SummaryJob) -> Self {
        Self {
            id: j.id,
            room_id: j.room_id.as_uuid(),
            version: j.version,
            status: j.status,
            trigger: j.trigger,
            model: j.model,
            prompt_version: j.prompt_version,
            content: j.content,
            error_code: j.error_code,
            error: j.error,
            attempts: j.attempts,
            tokens_in: j.tokens_in,
            tokens_out: j.tokens_out,
            created_at: j.created_at,
            started_at: j.started_at,
            completed_at: j.completed_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryResponse {
    /// `none` | `pending` | `running` | `completed` | `failed` (status of the latest version).
    pub status: &'static str,
    pub latest: Option<SummaryView>,
    /// The newest completed version when `latest` is not it (so the UI keeps showing content
    /// while a regenerate runs or after it failed).
    pub last_completed: Option<SummaryView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItemView {
    pub id: Uuid,
    pub room_id: Uuid,
    pub summary_id: Uuid,
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub status: String,
    pub source_segment_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ActionItem> for ActionItemView {
    fn from(a: ActionItem) -> Self {
        Self {
            id: a.id,
            room_id: a.room_id.as_uuid(),
            summary_id: a.summary_id,
            task: a.task,
            owner: a.owner,
            due: a.due,
            status: a.status,
            source_segment_ids: a.source_segment_ids,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[derive(Serialize)]
pub struct ItemsResponse {
    pub items: Vec<ActionItemView>,
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchItemRequest {
    pub task: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub owner: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub due: Option<Option<String>>,
    /// `open` | `done`.
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct WorkspaceItemsQuery {
    pub status: Option<String>,
    #[serde(default)]
    pub mine: bool,
    pub limit: Option<i64>,
}

// ------------------------------------------------------------------ access

async fn room_access(s: &State_, room: RoomId, auth: &AuthUser) -> AppResult<RoomAccessInfo> {
    let info = s
        .api
        .access
        .room_access(room, auth.user_id())
        .await?
        .ok_or_else(|| AppError::not_found("meeting not found"))?;
    if !info.can_view() {
        return Err(AppError::forbidden(
            "not allowed to see this meeting's summary",
        ));
    }
    Ok(info)
}

fn parse_status(s: &str) -> AppResult<&'static str> {
    match s {
        "open" => Ok("open"),
        "done" => Ok("done"),
        _ => Err(AppError::validation(
            "INVALID_STATUS",
            "status must be `open` or `done`",
        )),
    }
}

fn clean(v: Option<String>, max: usize, what: &str) -> AppResult<Option<String>> {
    let v = v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if v.as_deref().is_some_and(|s| s.chars().count() > max) {
        return Err(AppError::validation(
            "INVALID_REQUEST",
            format!("{what} must be at most {max} characters"),
        ));
    }
    Ok(v)
}

// ------------------------------------------------------------------ handlers

async fn get_summary(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<Json<SummaryResponse>> {
    let room = RoomId::from_uuid(room);
    let info = room_access(&s, room, &auth).await?;
    let repo = s.api.service.repo();
    let latest = repo
        .latest(room)
        .await?
        .filter(|j| j.workspace_id == info.workspace_id);
    let Some(latest) = latest else {
        return Ok(Json(SummaryResponse {
            status: "none",
            latest: None,
            last_completed: None,
        }));
    };
    let last_completed = if latest.status == SummaryStatus::Completed {
        None
    } else {
        repo.latest_completed(room).await?.map(SummaryView::from)
    };
    Ok(Json(SummaryResponse {
        status: latest.status.as_str(),
        latest: Some(latest.into()),
        last_completed,
    }))
}

async fn regenerate(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<(StatusCode, Json<SummaryView>)> {
    let room = RoomId::from_uuid(room);
    let info = room_access(&s, room, &auth).await?;
    if !info.can_manage() {
        return Err(AppError::forbidden(
            "only the host or a workspace admin can regenerate",
        ));
    }
    let job = s
        .api
        .service
        .regenerate(info.workspace_id, room, auth.user_id())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

async fn list_room_items(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
) -> AppResult<Json<ItemsResponse>> {
    let room = RoomId::from_uuid(room);
    let info = room_access(&s, room, &auth).await?;
    let items = s
        .api
        .service
        .repo()
        .items_for_room(info.workspace_id, room)
        .await?;
    Ok(Json(ItemsResponse {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

async fn patch_item(
    State(s): State<State_>,
    auth: AuthUser,
    Path((room, item_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<PatchItemRequest>,
) -> AppResult<Json<ActionItemView>> {
    let room = RoomId::from_uuid(room);
    let info = room_access(&s, room, &auth).await?;
    let task = match req.task {
        Some(t) => Some(
            clean(Some(t), 1000, "task")?
                .ok_or_else(|| AppError::validation("INVALID_REQUEST", "task must not be empty"))?,
        ),
        None => None,
    };
    let patch = ItemPatch {
        task,
        owner: req.owner.map(|o| clean(o, 200, "owner")).transpose()?,
        due: req.due.map(|d| clean(d, 200, "due")).transpose()?,
        status: req
            .status
            .as_deref()
            .map(parse_status)
            .transpose()?
            .map(str::to_owned),
    };
    let repo = s.api.service.repo();
    let existing = repo
        .item(info.workspace_id, item_id)
        .await?
        .filter(|i| i.room_id == room)
        .ok_or_else(|| AppError::not_found("action item not found"))?;
    let updated = repo
        .update_item(info.workspace_id, existing.id, &patch, s.api.clock.now())
        .await?
        .ok_or_else(|| AppError::not_found("action item not found"))?;
    Ok(Json(updated.into()))
}

async fn list_workspace_items(
    State(s): State<State_>,
    auth: AuthUser,
    Path(ws): Path<Uuid>,
    Query(q): Query<WorkspaceItemsQuery>,
) -> AppResult<Json<ItemsResponse>> {
    let ws = WorkspaceId::from_uuid(ws);
    let scope = s
        .api
        .access
        .workspace_scope(ws, auth.user_id())
        .await?
        .ok_or_else(|| AppError::forbidden("not a member of this workspace"))?;
    let admin = matches!(
        scope.role,
        warptalk_kernel::WorkspaceRole::Owner | warptalk_kernel::WorkspaceRole::Admin
    );
    let filter = ItemFilter {
        status: q
            .status
            .as_deref()
            .map(parse_status)
            .transpose()?
            .map(str::to_owned),
        room_ids: (!admin).then(|| scope.room_ids.iter().map(|r| r.as_uuid()).collect()),
        owner_names: q.mine.then(|| scope.names.clone()),
        limit: q.limit.unwrap_or(100).clamp(1, 500),
    };
    let items = s
        .api
        .service
        .repo()
        .items_for_workspace(ws, &filter)
        .await?;
    Ok(Json(ItemsResponse {
        items: items.into_iter().map(Into::into).collect(),
    }))
}
