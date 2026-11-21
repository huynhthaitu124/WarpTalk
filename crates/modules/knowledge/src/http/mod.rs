//! HTTP surface: `GET knowledge/search`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, WorkspaceId};
use warptalk_web::extract::{Json, Query};
use warptalk_web::{AuthUser, JwtKeys};

use crate::application::{
    AccessContext, DocumentAccess, KnowledgeService, SearchFilters, SearchMetrics,
};
use crate::domain::{Citation, SourceType};

#[derive(Clone)]
pub(crate) struct HttpState {
    pub service: Arc<KnowledgeService>,
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
