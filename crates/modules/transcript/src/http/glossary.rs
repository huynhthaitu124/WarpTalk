//! Glossary REST: workspace glossaries (`/glossaries`) and the system-admin global glossary
//! (`/admin/global-glossary`). Spec 06 section 5.2, spec 12 sections 3 and 5.7.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{FromRef, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys, SystemAdmin};

use crate::application::glossary_service::{
    CreateGlossary, GlobalInput, GlossaryService, TermInput, UpdateGlossary,
};
use crate::domain::glossary::{GlobalAudit, GlobalStatus, GlobalTerm, Glossary, GlossaryTerm};

#[derive(Clone)]
pub struct GlossaryState {
    pub service: Arc<GlossaryService>,
    pub jwt: JwtKeys,
}

impl FromRef<GlossaryState> for JwtKeys {
    fn from_ref(s: &GlossaryState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: GlossaryState) -> Router {
    Router::new()
        .route("/glossaries", post(create))
        .route("/glossaries/global", get(published_global))
        .route(
            "/glossaries/workspace/{workspace_id}",
            get(list_by_workspace),
        )
        .route("/glossaries/{id}", get(get_one).put(update).delete(remove))
        .route("/glossaries/{id}/terms", get(list_terms).post(add_term))
        .route("/glossaries/{id}/terms/bulk", post(import_terms))
        .route("/glossaries/{id}/export", get(export))
        .route(
            "/glossaries/{id}/terms/{term_id}",
            put(update_term).delete(delete_term),
        )
        .route(
            "/admin/global-glossary",
            get(global_list).post(global_create),
        )
        .route(
            "/admin/global-glossary/{id}",
            get(global_get).put(global_update).delete(global_delete),
        )
        .route("/admin/global-glossary/{id}/publish", post(global_publish))
        .route("/admin/global-glossary/{id}/archive", post(global_archive))
        .route("/admin/global-glossary/{id}/audits", get(global_audits))
        .with_state(state)
}

// ---------------------------------------------------------------- DTOs

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGlossaryRequest {
    workspace_id: WorkspaceId,
    name: String,
    description: Option<String>,
    source_language: String,
    target_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateGlossaryRequest {
    name: Option<String>,
    description: Option<String>,
    source_language: Option<String>,
    target_language: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TermRequest {
    source_term: String,
    target_term: String,
    #[serde(default)]
    case_sensitive: bool,
    notes: Option<String>,
}

impl From<TermRequest> for TermInput {
    fn from(t: TermRequest) -> Self {
        Self {
            source_term: t.source_term,
            target_term: t.target_term,
            case_sensitive: t.case_sensitive,
            notes: t.notes,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImportJson {
    csv: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalRequest {
    term: String,
    preferred_translation: String,
    source_language: Option<String>,
    target_language: Option<String>,
    business_domain: Option<String>,
    definition: Option<String>,
    usage_note: Option<String>,
    priority: Option<i32>,
}

impl From<GlobalRequest> for GlobalInput {
    fn from(r: GlobalRequest) -> Self {
        Self {
            term: r.term,
            preferred_translation: r.preferred_translation,
            source_language: r.source_language,
            target_language: r.target_language,
            business_domain: r.business_domain,
            definition: r.definition,
            usage_note: r.usage_note,
            priority: r.priority,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalListQuery {
    page: Option<i64>,
    page_size: Option<i64>,
    status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlossaryDto {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    description: Option<String>,
    source_language: String,
    target_language: String,
    term_count: i32,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
}

impl From<Glossary> for GlossaryDto {
    fn from(g: Glossary) -> Self {
        Self {
            id: g.id,
            workspace_id: g.workspace_id.as_uuid(),
            name: g.name,
            description: g.description,
            source_language: g.source_language,
            target_language: g.target_language,
            term_count: g.term_count,
            created_at: g.created_at,
            updated_at: g.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermDto {
    id: Uuid,
    glossary_id: Uuid,
    source_term: String,
    target_term: String,
    case_sensitive: bool,
    notes: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<GlossaryTerm> for TermDto {
    fn from(t: GlossaryTerm) -> Self {
        Self {
            id: t.id,
            glossary_id: t.glossary_id,
            source_term: t.source_term,
            target_term: t.target_term,
            case_sensitive: t.case_sensitive,
            notes: t.notes,
            created_at: t.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportDto {
    imported: usize,
    skipped: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobalTermDto {
    id: Uuid,
    term: String,
    preferred_translation: String,
    source_language: Option<String>,
    target_language: Option<String>,
    business_domain: Option<String>,
    definition: Option<String>,
    usage_note: Option<String>,
    priority: i32,
    status: &'static str,
    version: i32,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
}

impl From<GlobalTerm> for GlobalTermDto {
    fn from(t: GlobalTerm) -> Self {
        Self {
            id: t.id,
            term: t.term,
            preferred_translation: t.preferred_translation,
            source_language: t.source_language,
            target_language: t.target_language,
            business_domain: t.business_domain,
            definition: t.definition,
            usage_note: t.usage_note,
            priority: t.priority,
            status: t.status.as_str(),
            version: t.version,
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobalPageDto {
    items: Vec<GlobalTermDto>,
    total: i64,
    page: i64,
    page_size: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditDto {
    id: Uuid,
    term_id: Uuid,
    action: String,
    before_json: Option<serde_json::Value>,
    after_json: Option<serde_json::Value>,
    actor_user_id: Uuid,
    created_at: DateTime<Utc>,
}

impl From<GlobalAudit> for AuditDto {
    fn from(a: GlobalAudit) -> Self {
        Self {
            id: a.id,
            term_id: a.term_id,
            action: a.action,
            before_json: a.before_json,
            after_json: a.after_json,
            actor_user_id: a.actor_user_id,
            created_at: a.created_at,
        }
    }
}

// ---------------------------------------------------------------- workspace handlers

type ApiResult<T> = AppResult<Json<T>>;

async fn create(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Json(b): Json<CreateGlossaryRequest>,
) -> AppResult<(StatusCode, Json<GlossaryDto>)> {
    let g = s
        .service
        .create(
            auth.user_id(),
            CreateGlossary {
                workspace_id: b.workspace_id,
                name: b.name,
                description: b.description,
                source_language: b.source_language,
                target_language: b.target_language,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(g.into())))
}

async fn published_global(
    State(s): State<GlossaryState>,
    _auth: AuthUser,
) -> ApiResult<Vec<GlobalTermDto>> {
    let list = s.service.published_global().await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

async fn list_by_workspace(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
) -> ApiResult<Vec<GlossaryDto>> {
    let list = s.service.list_for_workspace(auth.user_id(), ws).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

async fn get_one(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<GlossaryDto> {
    Ok(Json(s.service.get(auth.user_id(), id).await?.into()))
}

async fn update(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateGlossaryRequest>,
) -> ApiResult<GlossaryDto> {
    let g = s
        .service
        .update(
            auth.user_id(),
            id,
            UpdateGlossary {
                name: b.name,
                description: b.description,
                source_language: b.source_language,
                target_language: b.target_language,
            },
        )
        .await?;
    Ok(Json(g.into()))
}

async fn remove(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    s.service.delete(auth.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_terms(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Vec<TermDto>> {
    let list = s.service.terms(auth.user_id(), id).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

async fn add_term(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(b): Json<TermRequest>,
) -> AppResult<(StatusCode, Json<TermDto>)> {
    let t = s.service.add_term(auth.user_id(), id, b.into()).await?;
    Ok((StatusCode::CREATED, Json(t.into())))
}

async fn update_term(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path((id, term_id)): Path<(Uuid, Uuid)>,
    Json(b): Json<TermRequest>,
) -> ApiResult<TermDto> {
    let t = s
        .service
        .update_term(auth.user_id(), id, term_id, b.into())
        .await?;
    Ok(Json(t.into()))
}

async fn delete_term(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path((id, term_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    s.service.delete_term(auth.user_id(), id, term_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Body: `text/csv` (or `text/plain`) lines `source,target`, or JSON `{ "csv": "..." }`.
async fn import_terms(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<ImportDto> {
    let ctype = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let csv = if ctype.starts_with("application/json") {
        serde_json::from_slice::<ImportJson>(&body)
            .map_err(|e| AppError::validation("INVALID_REQUEST", e.to_string()))?
            .csv
    } else {
        String::from_utf8(body.to_vec())
            .map_err(|_| AppError::validation("INVALID_CSV", "body must be UTF-8 text"))?
    };
    let r = s.service.import_csv(auth.user_id(), id, &csv).await?;
    Ok(Json(ImportDto {
        imported: r.imported,
        skipped: r.skipped,
    }))
}

async fn export(
    State(s): State<GlossaryState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let csv = s.service.export_csv(auth.user_id(), id).await?;
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"glossary-{id}.csv\""),
            ),
        ],
        csv,
    ))
}

// ---------------------------------------------------------------- global handlers

async fn global_list(
    State(s): State<GlossaryState>,
    _admin: SystemAdmin,
    Query(q): Query<GlobalListQuery>,
) -> ApiResult<GlobalPageDto> {
    let status = match q.status.as_deref() {
        None | Some("") => None,
        Some(v) => Some(GlobalStatus::parse(&v.to_ascii_lowercase()).ok_or_else(|| {
            AppError::validation(
                "INVALID_STATUS",
                "status must be draft, published or archived",
            )
        })?),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(50).clamp(1, 200);
    let (items, total) = s.service.global_list(status, page, page_size).await?;
    Ok(Json(GlobalPageDto {
        items: items.into_iter().map(Into::into).collect(),
        total,
        page,
        page_size,
    }))
}

async fn global_get(
    State(s): State<GlossaryState>,
    _admin: SystemAdmin,
    Path(id): Path<Uuid>,
) -> ApiResult<GlobalTermDto> {
    Ok(Json(s.service.global_get(id).await?.into()))
}

async fn global_create(
    State(s): State<GlossaryState>,
    admin: SystemAdmin,
    Json(b): Json<GlobalRequest>,
) -> AppResult<(StatusCode, Json<GlobalTermDto>)> {
    let t = s.service.global_create(admin.0.sub, b.into()).await?;
    Ok((StatusCode::CREATED, Json(t.into())))
}

async fn global_update(
    State(s): State<GlossaryState>,
    admin: SystemAdmin,
    Path(id): Path<Uuid>,
    Json(b): Json<GlobalRequest>,
) -> ApiResult<GlobalTermDto> {
    Ok(Json(
        s.service
            .global_update(admin.0.sub, id, b.into())
            .await?
            .into(),
    ))
}

async fn global_delete(
    State(s): State<GlossaryState>,
    admin: SystemAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    s.service.global_delete(admin.0.sub, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn global_publish(
    State(s): State<GlossaryState>,
    admin: SystemAdmin,
    Path(id): Path<Uuid>,
) -> ApiResult<GlobalTermDto> {
    Ok(Json(
        s.service.global_publish(admin.0.sub, id).await?.into(),
    ))
}

async fn global_archive(
    State(s): State<GlossaryState>,
    admin: SystemAdmin,
    Path(id): Path<Uuid>,
) -> ApiResult<GlobalTermDto> {
    Ok(Json(
        s.service.global_archive(admin.0.sub, id).await?.into(),
    ))
}

async fn global_audits(
    State(s): State<GlossaryState>,
    _admin: SystemAdmin,
    Path(id): Path<Uuid>,
) -> ApiResult<Vec<AuditDto>> {
    let list = s.service.global_audits(id).await?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}
