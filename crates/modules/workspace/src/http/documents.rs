//! Workspace document endpoints (multipart upload, list, get, download, patch, archive).

use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Page, PageRequest, UserId, WorkspaceId};
use warptalk_web::AuthUser;
use warptalk_web::extract::{Json, Path, Query};

use super::AppState;
use super::dto::map_page;
use crate::application::documents::{
    DocumentFilter, DocumentRecord, DocumentService, Download, PatchInput, UploadInput, too_large,
};

/// Multipart framing overhead allowed on top of the file size limit.
const MULTIPART_OVERHEAD: usize = 1024 * 1024;
/// Longest accepted text field (title, category, ...).
const MAX_FIELD_BYTES: usize = 4096;

pub fn routes(max_upload_bytes: u64) -> Router<AppState> {
    let body_limit = usize::try_from(max_upload_bytes)
        .unwrap_or(usize::MAX - MULTIPART_OVERHEAD)
        .saturating_add(MULTIPART_OVERHEAD);
    Router::new()
        .route(
            "/workspaces/{id}/documents",
            get(list)
                .post(upload)
                .layer(DefaultBodyLimit::max(body_limit)),
        )
        .route(
            "/workspaces/{id}/documents/{doc_id}",
            get(get_one).patch(patch_one).delete(archive_one),
        )
        .route(
            "/workspaces/{id}/documents/{doc_id}/download",
            get(download),
        )
}

/// Mounted instead of [`routes`] when no object store is configured.
pub fn unavailable_routes() -> Router<AppState> {
    async fn unavailable() -> AppError {
        AppError::coded(
            503,
            "STORAGE_NOT_CONFIGURED",
            "document storage is not configured",
        )
    }
    Router::new()
        .route(
            "/workspaces/{id}/documents",
            get(unavailable).post(unavailable),
        )
        .route(
            "/workspaces/{id}/documents/{doc_id}",
            get(unavailable).patch(unavailable).delete(unavailable),
        )
        .route(
            "/workspaces/{id}/documents/{doc_id}/download",
            get(unavailable),
        )
}

fn docs(s: &AppState) -> AppResult<&DocumentService> {
    s.documents.as_deref().ok_or_else(|| {
        AppError::coded(
            503,
            "STORAGE_NOT_CONFIGURED",
            "document storage is not configured",
        )
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDto {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub file_name: String,
    pub file_extension: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub category: String,
    pub access_level: &'static str,
    pub ai_retrieval_allowed: bool,
    /// True once the document is indexed and usable by the assistant.
    pub ai_eligible: bool,
    /// pending | completed | skipped | failed
    pub ingestion_status: String,
    pub ingestion_failure_reason: Option<String>,
    pub last_indexed_at: Option<DateTime<Utc>>,
    pub uploaded_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl From<DocumentRecord> for DocumentDto {
    fn from(d: DocumentRecord) -> Self {
        Self {
            id: d.id,
            workspace_id: d.workspace_id,
            title: d.title,
            file_name: d.file_name,
            file_extension: d.file_extension,
            mime_type: d.mime_type,
            size_bytes: d.size_bytes,
            category: d.category,
            access_level: d.access_level.as_str(),
            ai_retrieval_allowed: d.ai_retrieval_allowed,
            ai_eligible: d.ai_eligible,
            ingestion_status: d.ingestion_status,
            ingestion_failure_reason: d.ingestion_failure_reason,
            last_indexed_at: d.last_indexed_at,
            uploaded_by: d.uploaded_by,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub category: Option<String>,
    pub search: Option<String>,
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    /// `true`: answer `{ "url", "expiresAt" }` when the store can presign.
    #[serde(default)]
    pub presign: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchRequest {
    pub title: Option<String>,
    pub category: Option<String>,
    pub access_level: Option<String>,
    pub ai_retrieval_allowed: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadUrlDto {
    url: String,
    expires_at: DateTime<Utc>,
}

fn multipart_error(e: axum::extract::multipart::MultipartError) -> AppError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::coded(413, "FILE_TOO_LARGE", "File exceeds the size limit.")
    } else {
        AppError::validation("INVALID_MULTIPART", "malformed multipart/form-data body")
    }
}

fn parse_bool(name: &str, v: &str) -> AppResult<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Ok(true),
        "false" | "0" | "off" | "no" => Ok(false),
        _ => Err(AppError::validation(
            "INVALID_REQUEST",
            format!("{name} must be true or false"),
        )),
    }
}

async fn read_field(
    field: &mut axum::extract::multipart::Field<'_>,
    limit: usize,
) -> AppResult<Bytes> {
    let mut buf = BytesMut::new();
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        if buf.len() + chunk.len() > limit {
            return Err(AppError::validation(
                "INVALID_REQUEST",
                "multipart field too large",
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

pub async fn upload(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
    mut mp: Multipart,
) -> AppResult<(StatusCode, Json<DocumentDto>)> {
    let svc = docs(&s)?;
    let max = svc.max_upload_bytes();
    let max_len = usize::try_from(max).unwrap_or(usize::MAX);

    let mut file: Option<(String, Bytes)> = None;
    let mut input = UploadInput {
        file_name: String::new(),
        bytes: Bytes::new(),
        title: None,
        category: None,
        access_level: None,
        ai_retrieval_allowed: None,
    };
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "file" {
            let file_name = field.file_name().unwrap_or("document").to_owned();
            let mut buf = BytesMut::new();
            while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                if buf.len() + chunk.len() > max_len {
                    return Err(too_large(max));
                }
                buf.extend_from_slice(&chunk);
            }
            file = Some((file_name, buf.freeze()));
            continue;
        }
        let text_field = |b: Bytes| {
            String::from_utf8(b.to_vec()).map_err(|_| {
                AppError::validation("INVALID_REQUEST", format!("{name} is not UTF-8"))
            })
        };
        match name.as_str() {
            "title" => {
                input.title = Some(text_field(read_field(&mut field, MAX_FIELD_BYTES).await?)?)
            }
            "category" => {
                input.category = Some(text_field(read_field(&mut field, MAX_FIELD_BYTES).await?)?);
            }
            "accessLevel" => {
                input.access_level =
                    Some(text_field(read_field(&mut field, MAX_FIELD_BYTES).await?)?);
            }
            "aiRetrievalAllowed" => {
                let v = text_field(read_field(&mut field, MAX_FIELD_BYTES).await?)?;
                input.ai_retrieval_allowed = Some(parse_bool("aiRetrievalAllowed", &v)?);
            }
            _ => {}
        }
    }
    let (file_name, bytes) = file.ok_or_else(|| {
        AppError::validation("FILE_REQUIRED", "multipart field `file` is required")
    })?;
    input.file_name = file_name;
    input.bytes = bytes;
    let doc = svc.upload(ws, auth.user_id(), input).await?;
    Ok((StatusCode::CREATED, Json(doc.into())))
}

pub async fn list(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
    Query(paging): Query<PageRequest>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Page<DocumentDto>>> {
    let filter = DocumentFilter {
        category: q
            .category
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .map(|c| crate::domain::documents::parse_category(c).map(str::to_owned))
            .transpose()?,
        search: q.search,
    };
    let page = docs(&s)?.list(ws, auth.user_id(), &filter, &paging).await?;
    Ok(Json(map_page(page, DocumentDto::from)))
}

pub async fn get_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((ws, id)): Path<(WorkspaceId, Uuid)>,
) -> AppResult<Json<DocumentDto>> {
    Ok(Json(docs(&s)?.get(ws, auth.user_id(), id).await?.into()))
}

pub async fn patch_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((ws, id)): Path<(WorkspaceId, Uuid)>,
    Json(req): Json<PatchRequest>,
) -> AppResult<Json<DocumentDto>> {
    let doc = docs(&s)?
        .patch(
            ws,
            auth.user_id(),
            id,
            PatchInput {
                title: req.title,
                category: req.category,
                access_level: req.access_level,
                ai_retrieval_allowed: req.ai_retrieval_allowed,
            },
        )
        .await?;
    Ok(Json(doc.into()))
}

pub async fn archive_one(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((ws, id)): Path<(WorkspaceId, Uuid)>,
) -> AppResult<StatusCode> {
    docs(&s)?.archive(ws, auth.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `Content-Disposition: attachment` with an ASCII fallback and an RFC 5987 `filename*`.
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

pub async fn download(
    State(s): State<AppState>,
    auth: AuthUser,
    Path((ws, id)): Path<(WorkspaceId, Uuid)>,
    Query(q): Query<DownloadQuery>,
) -> AppResult<Response> {
    match docs(&s)?
        .download(ws, auth.user_id(), id, q.presign)
        .await?
    {
        Download::Url { url, expires_at } => {
            Ok(Json(DownloadUrlDto { url, expires_at }).into_response())
        }
        Download::File {
            bytes,
            mime,
            file_name,
        } => {
            let mut resp = bytes.into_response();
            let h = resp.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&mime) {
                h.insert(header::CONTENT_TYPE, v);
            }
            if let Ok(v) = HeaderValue::from_str(&content_disposition(&file_name)) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            Ok(resp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_is_header_safe() {
        let d = content_disposition("Résumé \"v1\".pdf");
        assert!(
            d.starts_with("attachment; filename=\"R_sum_ v1.pdf\""),
            "{d}"
        );
        assert!(
            d.contains("filename*=UTF-8''R%C3%A9sum%C3%A9%20%22v1%22.pdf"),
            "{d}"
        );
    }
}
