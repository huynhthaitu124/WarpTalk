//! HTTP surface: `GET knowledge/search` and chunk management under `workspaces/{id}/knowledge`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};

use crate::application::{
    AccessContext, AccessScope, ChunkPage, ChunkService, ChunkView, DocumentAccess,
    KnowledgeService, ListChunks, PatchChunk, SearchFilters, SearchMetrics,
};
use crate::domain::{Citation, SourceType};

#[derive(Clone)]
pub(crate) struct HttpState {
    pub service: Arc<KnowledgeService>,
    pub chunks: Arc<ChunkService>,
    pub access: Arc<dyn DocumentAccess>,
    pub jwt: JwtKeys,
}

impl FromRef<HttpState> for JwtKeys {
    fn from_ref(s: &HttpState) -> JwtKeys {
        s.jwt.clone()
    }
}

pub(crate) fn router(state: HttpState) -> Router {
    Router::new()
        .route("/knowledge/search", get(search))
        .route("/workspaces/{workspace_id}/knowledge", get(list_chunks))
        .route(
            "/workspaces/{workspace_id}/knowledge/{chunk_id}",
            patch(patch_chunk).delete(delete_chunk),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    workspace_id: Uuid,
    q: String,
    /// Comma-separated source types.
    source_type: Option<String>,
    page: Option<u32>,
    source_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HitDto {
    chunk_id: Uuid,
    score: f32,
    dense_score: f32,
    source_type: SourceType,
    source_id: String,
    text: String,
    citation: Citation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    chunks: Vec<HitDto>,
    metrics: SearchMetrics,
}

async fn search(
    State(s): State<HttpState>,
    user: AuthUser,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<SearchResponse>> {
    let mut source_types = Vec::new();
    for t in q
        .source_type
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        source_types.push(
            t.parse::<SourceType>()
                .map_err(|m| AppError::validation("INVALID_SOURCE_TYPE", m))?,
        );
    }
    if q.q.trim().is_empty() {
        return Err(AppError::validation("INVALID_QUERY", "q must not be empty"));
    }
    let workspace = WorkspaceId::from_uuid(q.workspace_id);
    // Fails closed: not a member or allowlist unavailable -> error, nothing is searched.
    let scope = s.access.access_scope(workspace, user.user_id()).await?;
    // Owner/Admin are unscoped within their workspace; members are limited to their allowlists.
    let unscoped = scope.role.rank() >= warptalk_kernel::WorkspaceRole::Admin.rank();
    let ctx = AccessContext {
        workspace,
        user: user.user_id(),
        role: scope.role,
        allowed_document_ids: (!unscoped).then_some(scope.allowed_document_ids),
        allowed_meeting_ids: (!unscoped).then_some(scope.allowed_meeting_ids),
    };
    let r = s
        .service
        .search(
            &ctx,
            &q.q,
            &SearchFilters {
                source_types,
                page: q.page,
                source_id: q.source_id,
                limit: q.limit,
            },
        )
        .await?;
    Ok(Json(SearchResponse {
        chunks: r
            .chunks
            .into_iter()
            .map(|h| HitDto {
                chunk_id: h.chunk_id,
                score: h.score,
                dense_score: h.dense_score,
                source_type: h.payload.source_type,
                source_id: h.payload.source_id,
                text: h.payload.text,
                citation: h.citation,
            })
            .collect(),
        metrics: r.metrics,
    }))
}

fn is_admin(scope: &AccessScope) -> bool {
    scope.role.rank() >= warptalk_kernel::WorkspaceRole::Admin.rank()
}

/// Membership is enforced by the access port (fails closed); editing needs owner/admin.
async fn require_admin(s: &HttpState, workspace: Uuid, user: &AuthUser) -> AppResult<()> {
    let scope = s
        .access
        .access_scope(WorkspaceId::from_uuid(workspace), user.user_id())
        .await?;
    if is_admin(&scope) {
        Ok(())
    } else {
        Err(AppError::forbidden(
            "only workspace owners and admins can manage knowledge",
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    source_type: Option<String>,
    fact_category: Option<String>,
    cursor: Option<String>,
    page_size: Option<u32>,
}

async fn list_chunks(
    State(s): State<HttpState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ChunkPage>> {
    let scope = s
        .access
        .access_scope(WorkspaceId::from_uuid(workspace_id), user.user_id())
        .await?;
    // Owner/Admin see the whole workspace; members only what their allowlists admit.
    let unscoped = is_admin(&scope);
    let page = s
        .chunks
        .list(
            workspace_id,
            ListChunks {
                source_type: q.source_type,
                fact_category: q.fact_category,
                cursor: q.cursor,
                page_size: q.page_size,
                allowed_document_ids: (!unscoped).then_some(scope.allowed_document_ids),
                allowed_meeting_ids: (!unscoped).then_some(scope.allowed_meeting_ids),
            },
        )
        .await?;
    Ok(Json(page))
}

/// Distinguishes an absent key (`None`) from an explicit `null` (`Some(None)`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchBody {
    #[serde(default, deserialize_with = "present")]
    fact: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    fact_category: Option<Option<String>>,
}

async fn patch_chunk(
    State(s): State<HttpState>,
    user: AuthUser,
    Path((workspace_id, chunk_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<ChunkView>> {
    require_admin(&s, workspace_id, &user).await?;
    let v = s
        .chunks
        .patch(
            workspace_id,
            chunk_id,
            PatchChunk {
                fact: body.fact,
                fact_category: body.fact_category,
            },
        )
        .await?;
    Ok(Json(v))
}

async fn delete_chunk(
    State(s): State<HttpState>,
    user: AuthUser,
    Path((workspace_id, chunk_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    require_admin(&s, workspace_id, &user).await?;
    s.chunks.delete(workspace_id, chunk_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
