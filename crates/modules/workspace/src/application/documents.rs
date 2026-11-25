//! Workspace documents: upload / list / get / download / patch / archive, and the bridge to the
//! knowledge index (spec 06 section 2.4, spec 14 sections 2-4).
//!
//! Indexing is asynchronous: this module publishes `embedding:index_requests` (and
//! `knowledge:purge_requests`) and later applies `embedding:index_results` to the row
//! ([`DocumentService::apply_index_result`]).
//!
//! Text extraction gap: `txt`/`md`/`csv` ship their text in the request (`content.pages`).
//! `pdf`/`docx` are sent without content and with `needs_extraction = true` plus the object key;
//! the knowledge `TextExtractor`/OCR port is expected to fetch and extract them. Until it does,
//! the knowledge module indexes 0 chunks and the document ends as `skipped`
//! (`no_indexable_content`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{
    AppError, AppResult, Clock, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole,
};
use warptalk_storage::{ObjectStore, StorageError};

use super::ports::WorkspaceRepo;
use crate::domain::documents::{
    self, AccessLevel, DocType, ReindexScope, authorize_manage, decode_text, ingestion,
    visible_levels,
};

/// Stream of purge requests consumed by the knowledge module.
pub const KNOWLEDGE_PURGE_REQUESTS: &str = "knowledge:purge_requests";

/// Ids handed to the knowledge allowlist (spec 14 section 4.2 max).
pub const MAX_ALLOWLIST_IDS: i64 = 500;

#[derive(Debug, Clone)]
pub struct DocumentsConfig {
    pub max_upload_bytes: u64,
    /// Lifetime of presigned download links.
    pub download_url_ttl: Duration,
}

impl Default for DocumentsConfig {
    fn default() -> Self {
        Self {
            max_upload_bytes: documents::DEFAULT_MAX_UPLOAD_MB * 1024 * 1024,
            download_url_ttl: Duration::from_secs(300),
        }
    }
}

impl DocumentsConfig {
    /// `DOCUMENT_MAX_UPLOAD_MB` (default 20), `DOCUMENT_DOWNLOAD_URL_TTL_SECONDS` (default 300).
    pub fn from_env() -> Self {
        let num = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
        };
        let d = Self::default();
        Self {
            max_upload_bytes: num("DOCUMENT_MAX_UPLOAD_MB")
                .map_or(d.max_upload_bytes, |mb| mb.saturating_mul(1024 * 1024)),
            download_url_ttl: num("DOCUMENT_DOWNLOAD_URL_TTL_SECONDS")
                .map_or(d.download_url_ttl, Duration::from_secs),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentRecord {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub file_name: String,
    pub file_extension: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_provider: String,
    pub storage_key: String,
    pub category: String,
    pub access_level: AccessLevel,
    pub ai_retrieval_allowed: bool,
    pub ai_eligible: bool,
    pub ingestion_status: String,
    pub ingestion_failure_reason: Option<String>,
    pub last_indexed_at: Option<DateTime<Utc>>,
    pub uploaded_by: Option<UserId>,
    pub revision: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewDocument {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub file_name: String,
    pub file_extension: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_provider: String,
    pub storage_key: String,
    pub category: String,
    pub access_level: AccessLevel,
    pub ai_retrieval_allowed: bool,
    pub ingestion_status: String,
    pub ingestion_failure_reason: Option<String>,
    pub uploaded_by: UserId,
    pub now: DateTime<Utc>,
}

/// Partial update; `None` leaves a column unchanged.
#[derive(Debug, Clone, Default)]
pub struct DocumentChanges {
    pub title: Option<String>,
    pub category: Option<String>,
    pub access_level: Option<AccessLevel>,
    pub ai_retrieval_allowed: Option<bool>,
    pub ai_eligible: Option<bool>,
    /// New ingestion status with its failure reason (set to NULL when `None`).
    pub ingestion: Option<(String, Option<String>)>,
    pub bump_revision: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentFilter {
    pub category: Option<String>,
    pub search: Option<String>,
}

/// What the knowledge index reported for a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOutcome {
    Indexed {
        chunks: u32,
        version: String,
    },
    Blocked {
        reason: String,
    },
    Failed {
        reason: String,
    },
    /// Vectors removed (archive / purge).
    Deleted,
}

#[async_trait]
pub trait DocumentRepo: Send + Sync {
    async fn insert(&self, doc: NewDocument) -> AppResult<DocumentRecord>;
    /// Any status (archived included); scoped to the workspace.
    async fn get(&self, ws: WorkspaceId, id: Uuid) -> AppResult<Option<DocumentRecord>>;
    /// Active documents whose access level is in `levels`, newest first.
    async fn list(
        &self,
        ws: WorkspaceId,
        levels: &[AccessLevel],
        filter: &DocumentFilter,
        page: &PageRequest,
    ) -> AppResult<Page<DocumentRecord>>;
    async fn update(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        changes: &DocumentChanges,
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>>;
    /// Soft delete: status `archived`, `deleted_at/by`, not AI eligible. `false` if the document
    /// is missing or already archived.
    async fn archive(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<bool>;
    /// Applies an index result unless it is stale (`revision` older than the row's). Archived
    /// rows keep their status. Returns the updated row when something was applied.
    async fn apply_index_result(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        revision: Option<i32>,
        outcome: &IndexOutcome,
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>>;
    /// Ids of active, AI-eligible, indexed documents visible at `levels` (most recently updated
    /// first).
    async fn ai_retrievable_ids(
        &self,
        ws: WorkspaceId,
        levels: &[AccessLevel],
        limit: i64,
    ) -> AppResult<Vec<Uuid>>;
    /// Atomically moves an active, AI-allowed document whose ingestion status is in `statuses`
    /// to `pending` with a bumped revision (so a concurrent second reindex finds nothing to
    /// claim). `None` when the document is not (or no longer) in one of those states.
    async fn begin_reindex(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        statuses: &[&str],
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>>;
    /// Ids of active, AI-allowed documents in `statuses` (oldest update first, at most `limit`)
    /// and the total number of such documents.
    async fn reindex_candidates(
        &self,
        ws: WorkspaceId,
        statuses: &[&str],
        limit: i64,
    ) -> AppResult<(Vec<Uuid>, i64)>;
    async fn audit(
        &self,
        ws: WorkspaceId,
        doc: Uuid,
        actor: Option<UserId>,
        action: &str,
        metadata: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
}

/// Wire shape of an `embedding:index_requests` entry for a document (spec 12 section 6.2; the
/// knowledge `IndexRequest` decodes the JSON `data` field and ignores the extra fields).
#[derive(Debug, Clone, Serialize)]
pub struct DocumentIndexRequest {
    pub job_id: String,
    pub workspace_id: Uuid,
    pub collection_id: String,
    pub source_type: &'static str,
    pub source_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    pub ai_retrieval_allowed: bool,
    pub retention_state: &'static str,
    /// `deleted` makes the indexer purge the document's vectors.
    pub deletion_state: &'static str,
    pub revision: String,
    pub timestamp_ms: i64,
    /// pdf/docx: the text is not in the request; the extractor must read `object_key`.
    pub needs_extraction: bool,
    pub object_key: String,
    pub storage_provider: String,
    pub mime_type: String,
    pub file_name: String,
}

impl DocumentIndexRequest {
    /// `bytes`: file content, needed to embed the text of plain-text types.
    pub fn new(doc: &DocumentRecord, bytes: Option<&[u8]>, deleted: bool, now_ms: i64) -> Self {
        let plain =
            DocType::from_extension(&doc.file_extension).is_some_and(DocType::is_plain_text);
        let content = (plain && !deleted)
            .then(|| bytes.and_then(decode_text))
            .flatten()
            .map(|text| json!({ "type": "document", "pages": [{ "page": null, "text": text }] }));
        Self {
            job_id: format!("{}:{}", doc.id, doc.revision),
            workspace_id: doc.workspace_id.as_uuid(),
            collection_id: format!("workspace_{}", doc.workspace_id),
            source_type: "document",
            source_id: doc.id.to_string(),
            title: doc.title.clone(),
            needs_extraction: !plain && !deleted,
            content,
            ai_retrieval_allowed: doc.ai_retrieval_allowed,
            retention_state: "active",
            deletion_state: if deleted { "deleted" } else { "active" },
            revision: doc.revision.to_string(),
            timestamp_ms: now_ms,
            object_key: doc.storage_key.clone(),
            storage_provider: doc.storage_provider.clone(),
            mime_type: doc.mime_type.clone(),
            file_name: doc.file_name.clone(),
        }
    }
}

#[async_trait]
pub trait DocumentIndexPublisher: Send + Sync {
    async fn publish_index(&self, req: &DocumentIndexRequest) -> AppResult<()>;
    /// `knowledge:purge_requests` for one document.
    async fn publish_purge(&self, ws: WorkspaceId, doc: Uuid, reason: &str) -> AppResult<()>;
}

/// Drops everything (no event bus configured).
pub struct NoopDocumentIndexPublisher;

#[async_trait]
impl DocumentIndexPublisher for NoopDocumentIndexPublisher {
    async fn publish_index(&self, _req: &DocumentIndexRequest) -> AppResult<()> {
        Ok(())
    }

    async fn publish_purge(&self, _ws: WorkspaceId, _doc: Uuid, _reason: &str) -> AppResult<()> {
        Ok(())
    }
}

pub struct UploadInput {
    pub file_name: String,
    pub bytes: Bytes,
    pub title: Option<String>,
    pub category: Option<String>,
    pub access_level: Option<String>,
    pub ai_retrieval_allowed: Option<bool>,
}

#[derive(Debug, Default)]
pub struct PatchInput {
    pub title: Option<String>,
    pub category: Option<String>,
    pub access_level: Option<String>,
    pub ai_retrieval_allowed: Option<bool>,
}

/// Result of reindexing one document.
#[derive(Debug, Clone)]
pub struct ReindexOutcome {
    pub document: DocumentRecord,
    /// False when the document was already `pending` (nothing re-published).
    pub queued: bool,
}

/// Result of a bulk reindex.
#[derive(Debug, Clone, Default)]
pub struct BatchReindexOutcome {
    /// Documents whose index request was re-published.
    pub queued: Vec<Uuid>,
    /// Claimed by a concurrent reindex / no longer in the requested state.
    pub not_eligible: u32,
    /// Claimed but the request could not be published (document is `failed` again).
    pub failed: Vec<Uuid>,
    /// Matching documents beyond this batch (call again).
    pub remaining: i64,
}

/// Default / maximum documents per bulk reindex call.
pub const REINDEX_DEFAULT_BATCH: i64 = 50;
pub const REINDEX_MAX_BATCH: i64 = 200;

pub enum Download {
    Url {
        url: String,
        expires_at: DateTime<Utc>,
    },
    File {
        bytes: Bytes,
        mime: String,
        file_name: String,
    },
}

pub struct DocumentService {
    workspaces: Arc<dyn WorkspaceRepo>,
    repo: Arc<dyn DocumentRepo>,
    store: Arc<dyn ObjectStore>,
    publisher: Arc<dyn DocumentIndexPublisher>,
    clock: Arc<dyn Clock>,
    cfg: DocumentsConfig,
}

fn storage_err(e: StorageError) -> AppError {
    match e {
        StorageError::NotFound(_) => AppError::not_found("document file not found"),
        other => {
            tracing::error!(error = %other, "object storage failure");
            AppError::unavailable("file storage unavailable")
        }
    }
}

fn not_found() -> AppError {
    AppError::not_found("document not found")
}

impl DocumentService {
    pub fn new(
        workspaces: Arc<dyn WorkspaceRepo>,
        repo: Arc<dyn DocumentRepo>,
        store: Arc<dyn ObjectStore>,
        publisher: Arc<dyn DocumentIndexPublisher>,
        clock: Arc<dyn Clock>,
        cfg: DocumentsConfig,
    ) -> Self {
        Self {
            workspaces,
            repo,
            store,
            publisher,
            clock,
            cfg,
        }
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.cfg.max_upload_bytes
    }

    /// Non-members and inactive workspaces yield `NotFound` (BR-97).
    async fn role(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRole> {
        self.workspaces
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::not_found("workspace not found"))
    }

    /// An active document the role may see; anything else is `NotFound` (no existence leak).
    async fn visible(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        role: WorkspaceRole,
    ) -> AppResult<DocumentRecord> {
        self.repo
            .get(ws, id)
            .await?
            .filter(|d| d.status == "active" && d.access_level.visible_to(role))
            .ok_or_else(not_found)
    }

    async fn audit(&self, doc: &DocumentRecord, actor: UserId, action: &str, meta: Value) {
        if let Err(e) = self
            .repo
            .audit(
                doc.workspace_id,
                doc.id,
                Some(actor),
                action,
                meta,
                self.clock.now(),
            )
            .await
        {
            tracing::warn!(document = %doc.id, action, error = %e, "document audit failed");
        }
    }

    pub async fn upload(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        input: UploadInput,
    ) -> AppResult<DocumentRecord> {
        authorize_manage(self.role(ws, actor).await?)?;

        let file_name = documents::sanitize_file_name(&input.file_name);
        let doc_type = DocType::from_file_name(&file_name).ok_or_else(|| {
            AppError::coded(
                415,
                "UNSUPPORTED_FILE_TYPE",
                "Unsupported file type: allowed types are pdf, docx, txt, md, csv, png, jpg and webp.",
            )
        })?;
        let size = input.bytes.len() as u64;
        if size == 0 {
            return Err(AppError::validation("EMPTY_FILE", "the file is empty"));
        }
        if size > self.cfg.max_upload_bytes {
            return Err(too_large(self.cfg.max_upload_bytes));
        }
        if !doc_type.content_matches(&input.bytes) {
            return Err(AppError::coded(
                415,
                "FILE_CONTENT_MISMATCH",
                "The file content does not match its extension.",
            ));
        }
        let title = match input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(t) => documents::validate_title(t)?,
            None => documents::validate_title(&documents::title_from_file_name(&file_name))
                .unwrap_or_else(|_| "Untitled".to_owned()),
        };
        let category = match input.category.as_deref().filter(|c| !c.trim().is_empty()) {
            Some(c) => documents::parse_category(c)?,
            None => "GENERAL",
        };
        let access_level = match input
            .access_level
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        {
            Some(a) => AccessLevel::parse(a)?,
            None => AccessLevel::MembersOnly,
        };
        let ai_allowed = input.ai_retrieval_allowed.unwrap_or(true);

        let id = Uuid::now_v7();
        let key = format!(
            "workspaces/{ws}/documents/{id}/original.{}",
            doc_type.extension()
        );
        self.store
            .put(&key, input.bytes.clone(), doc_type.mime())
            .await
            .map_err(storage_err)?;

        let (status, reason) = if ai_allowed {
            (ingestion::PENDING, None)
        } else {
            (
                ingestion::SKIPPED,
                Some("ai_retrieval_not_allowed".to_owned()),
            )
        };
        let inserted = self
            .repo
            .insert(NewDocument {
                id,
                workspace_id: ws,
                title,
                file_name,
                file_extension: doc_type.extension().to_owned(),
                mime_type: doc_type.mime().to_owned(),
                size_bytes: input.bytes.len() as i64,
                storage_provider: self.store.provider().to_owned(),
                storage_key: key.clone(),
                category: category.to_owned(),
                access_level,
                ai_retrieval_allowed: ai_allowed,
                ingestion_status: status.to_owned(),
                ingestion_failure_reason: reason,
                uploaded_by: actor,
                now: self.clock.now(),
            })
            .await;
        let doc = match inserted {
            Ok(d) => d,
            Err(e) => {
                if let Err(se) = self.store.delete(&key).await {
                    tracing::warn!(error = %se, key, "could not remove orphaned object");
                }
                return Err(e);
            }
        };
        self.audit(
            &doc,
            actor,
            "UploadDocument",
            json!({ "fileName": doc.file_name, "sizeBytes": doc.size_bytes }),
        )
        .await;

        if ai_allowed {
            return Ok(self.publish_index(doc, Some(&input.bytes)).await);
        }
        Ok(doc)
    }

    /// Publishes the index request; on failure the row becomes `failed` /
    /// `embedding_publish_failed` (no automatic retry; spec 14 section 9) and is returned as is.
    async fn publish_index(&self, doc: DocumentRecord, bytes: Option<&[u8]>) -> DocumentRecord {
        let req =
            DocumentIndexRequest::new(&doc, bytes, false, self.clock.now().timestamp_millis());
        match self.publisher.publish_index(&req).await {
            Ok(()) => doc,
            Err(e) => {
                tracing::error!(document = %doc.id, error = %e, "index request publish failed");
                self.mark_failed(doc, "embedding_publish_failed").await
            }
        }
    }

    async fn mark_failed(&self, doc: DocumentRecord, reason: &str) -> DocumentRecord {
        let changes = DocumentChanges {
            ai_eligible: Some(false),
            ingestion: Some((ingestion::FAILED.to_owned(), Some(reason.to_owned()))),
            ..Default::default()
        };
        match self
            .repo
            .update(doc.workspace_id, doc.id, &changes, self.clock.now())
            .await
        {
            Ok(Some(updated)) => updated,
            _ => doc,
        }
    }

    pub async fn list(
        &self,
        ws: WorkspaceId,
        user: UserId,
        filter: &DocumentFilter,
        page: &PageRequest,
    ) -> AppResult<Page<DocumentRecord>> {
        let role = self.role(ws, user).await?;
        self.repo
            .list(ws, &visible_levels(role), filter, page)
            .await
    }

    pub async fn get(&self, ws: WorkspaceId, user: UserId, id: Uuid) -> AppResult<DocumentRecord> {
        let role = self.role(ws, user).await?;
        self.visible(ws, id, role).await
    }

    /// `presign`: ask for a direct URL when the store supports it; otherwise (or when not
    /// asked) the file content is returned for streaming.
    pub async fn download(
        &self,
        ws: WorkspaceId,
        user: UserId,
        id: Uuid,
        presign: bool,
    ) -> AppResult<Download> {
        let role = self.role(ws, user).await?;
        let doc = self.visible(ws, id, role).await?;
        if presign
            && let Some(url) = self
                .store
                .presign_get(&doc.storage_key, self.cfg.download_url_ttl)
                .await
                .map_err(storage_err)?
        {
            let ttl = chrono::Duration::from_std(self.cfg.download_url_ttl)
                .unwrap_or_else(|_| chrono::Duration::seconds(300));
            return Ok(Download::Url {
                url,
                expires_at: self.clock.now() + ttl,
            });
        }
        let bytes = self
            .store
            .get(&doc.storage_key)
            .await
            .map_err(storage_err)?;
        Ok(Download::File {
            bytes,
            mime: doc.mime_type,
            file_name: doc.file_name,
        })
    }

    pub async fn patch(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        id: Uuid,
        input: PatchInput,
    ) -> AppResult<DocumentRecord> {
        let role = self.role(ws, actor).await?;
        authorize_manage(role)?;
        let before = self.visible(ws, id, role).await?;

        let title = input
            .title
            .as_deref()
            .map(documents::validate_title)
            .transpose()?;
        let category = input
            .category
            .as_deref()
            .map(documents::parse_category)
            .transpose()?
            .map(str::to_owned);
        let access_level = input
            .access_level
            .as_deref()
            .map(AccessLevel::parse)
            .transpose()?;

        let title_changed = title.as_ref().is_some_and(|t| *t != before.title);
        let ai_after = input
            .ai_retrieval_allowed
            .unwrap_or(before.ai_retrieval_allowed);
        let ai_changed = ai_after != before.ai_retrieval_allowed;
        // Access level and category are applied at query time (allowlist): no re-index needed.
        let reindex = ai_after && (title_changed || ai_changed);
        let purge = ai_changed && !ai_after;

        let mut changes = DocumentChanges {
            title,
            category,
            access_level,
            ai_retrieval_allowed: input.ai_retrieval_allowed,
            ..Default::default()
        };
        if reindex {
            changes.bump_revision = true;
            changes.ai_eligible = Some(false);
            changes.ingestion = Some((ingestion::PENDING.to_owned(), None));
        } else if purge {
            changes.ai_eligible = Some(false);
            changes.ingestion = Some((
                ingestion::SKIPPED.to_owned(),
                Some("ai_retrieval_not_allowed".to_owned()),
            ));
        }
        let mut doc = self
            .repo
            .update(ws, id, &changes, self.clock.now())
            .await?
            .ok_or_else(not_found)?;
        self.audit(
            &doc,
            actor,
            "UpdateDocument",
            json!({
                "title": changes.title, "category": changes.category,
                "accessLevel": changes.access_level.map(AccessLevel::as_str),
                "aiRetrievalAllowed": changes.ai_retrieval_allowed,
            }),
        )
        .await;

        if purge {
            self.purge(&doc, "ai_retrieval_disallowed").await;
            // Also lets the indexer answer `blocked` for the current revision.
            let req =
                DocumentIndexRequest::new(&doc, None, false, self.clock.now().timestamp_millis());
            if let Err(e) = self.publisher.publish_index(&req).await {
                tracing::warn!(document = %doc.id, error = %e, "blocked index request not published");
            }
        } else if reindex {
            match self.store.get(&doc.storage_key).await {
                Ok(bytes) => doc = self.publish_index(doc, Some(&bytes)).await,
                Err(e) => {
                    tracing::error!(document = %doc.id, error = %e, "cannot re-read document for indexing");
                    doc = self.mark_failed(doc, "storage_unavailable").await;
                }
            }
        }
        Ok(doc)
    }

    /// Re-publishes the index request of one document (owner/admin). Skipped, failed and
    /// completed documents get a new revision and go back to `pending`; a document that is
    /// already `pending` is left alone (`queued = false`), so repeating the call is harmless.
    pub async fn reindex(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        id: Uuid,
    ) -> AppResult<ReindexOutcome> {
        let role = self.role(ws, actor).await?;
        authorize_manage(role)?;
        let doc = self.visible(ws, id, role).await?;
        if !doc.ai_retrieval_allowed {
            return Err(AppError::coded(
                409,
                "AI_RETRIEVAL_NOT_ALLOWED",
                "AI retrieval is switched off for this document",
            ));
        }
        match self
            .reindex_claimed(doc.clone(), actor, ReindexScope::All)
            .await?
        {
            Some((document, _published)) => Ok(ReindexOutcome {
                document,
                queued: true,
            }),
            None => Ok(ReindexOutcome {
                document: self.visible(ws, id, role).await.unwrap_or(doc),
                queued: false,
            }),
        }
    }

    /// Claims `doc` (status in `scope`) and publishes its index request. `Ok(None)` = could not
    /// claim. The bool is false when publishing failed (the row is then `failed`).
    async fn reindex_claimed(
        &self,
        doc: DocumentRecord,
        actor: UserId,
        scope: ReindexScope,
    ) -> AppResult<Option<(DocumentRecord, bool)>> {
        let Some(claimed) = self
            .repo
            .begin_reindex(doc.workspace_id, doc.id, scope.statuses(), self.clock.now())
            .await?
        else {
            return Ok(None);
        };
        self.audit(
            &claimed,
            actor,
            "ReindexDocument",
            json!({ "previousStatus": doc.ingestion_status, "previousReason": doc.ingestion_failure_reason }),
        )
        .await;
        match self.store.get(&claimed.storage_key).await {
            Ok(bytes) => {
                let after = self.publish_index(claimed.clone(), Some(&bytes)).await;
                let ok = after.ingestion_status == ingestion::PENDING;
                Ok(Some((after, ok)))
            }
            Err(e) => {
                tracing::error!(document = %claimed.id, error = %e, "cannot re-read document for reindex");
                Ok(Some((
                    self.mark_failed(claimed, "storage_unavailable").await,
                    false,
                )))
            }
        }
    }

    /// Bulk reindex of up to `limit` documents in `scope` (owner/admin). Bounded; the response
    /// says how many matching documents remain. Already-pending documents are never touched,
    /// so overlapping calls do not double-publish.
    pub async fn reindex_batch(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        scope: ReindexScope,
        limit: Option<i64>,
    ) -> AppResult<BatchReindexOutcome> {
        authorize_manage(self.role(ws, actor).await?)?;
        let limit = limit
            .unwrap_or(REINDEX_DEFAULT_BATCH)
            .clamp(1, REINDEX_MAX_BATCH);
        let (ids, total) = self
            .repo
            .reindex_candidates(ws, scope.statuses(), limit)
            .await?;
        let mut out = BatchReindexOutcome {
            remaining: (total - ids.len() as i64).max(0),
            ..Default::default()
        };
        for id in ids {
            let Some(doc) = self.repo.get(ws, id).await? else {
                out.not_eligible += 1;
                continue;
            };
            match self.reindex_claimed(doc, actor, scope).await? {
                Some((_, true)) => out.queued.push(id),
                Some((_, false)) => out.failed.push(id),
                None => out.not_eligible += 1,
            }
        }
        Ok(out)
    }

    /// Best-effort purge of the document's vectors (failure is logged; the operation succeeds).
    async fn purge(&self, doc: &DocumentRecord, reason: &str) {
        if let Err(e) = self
            .publisher
            .publish_purge(doc.workspace_id, doc.id, reason)
            .await
        {
            tracing::warn!(document = %doc.id, error = %e, "purge request not published");
        }
    }

    /// Soft delete. Vectors are purged; the stored object is kept (retention).
    pub async fn archive(&self, ws: WorkspaceId, actor: UserId, id: Uuid) -> AppResult<()> {
        let role = self.role(ws, actor).await?;
        authorize_manage(role)?;
        let doc = self.visible(ws, id, role).await?;
        if !self.repo.archive(ws, id, actor, self.clock.now()).await? {
            return Err(not_found());
        }
        self.audit(&doc, actor, "ArchiveDocument", json!({})).await;
        self.purge(&doc, "archived").await;
        let req = DocumentIndexRequest::new(&doc, None, true, self.clock.now().timestamp_millis());
        if let Err(e) = self.publisher.publish_index(&req).await {
            tracing::warn!(document = %id, error = %e, "deletion index request not published");
        }
        Ok(())
    }

    /// Applies an `embedding:index_results` entry (called by the worker's result handler).
    /// Returns false when the document is unknown or the result is stale.
    pub async fn apply_index_result(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        revision: Option<i32>,
        outcome: &IndexOutcome,
    ) -> AppResult<bool> {
        apply_index_result(&*self.repo, &*self.clock, ws, id, revision, outcome).await
    }

    /// Document ids a `role` may have the assistant answer from (spec 14 section 4.2).
    pub async fn ai_retrievable_ids(
        &self,
        ws: WorkspaceId,
        role: WorkspaceRole,
    ) -> AppResult<Vec<Uuid>> {
        ai_retrievable_ids(&*self.repo, ws, role).await
    }
}

pub fn too_large(max: u64) -> AppError {
    AppError::coded(
        413,
        "FILE_TOO_LARGE",
        format!("File exceeds the {} limit.", documents::format_size(max)),
    )
}

pub(crate) async fn ai_retrievable_ids(
    repo: &dyn DocumentRepo,
    ws: WorkspaceId,
    role: WorkspaceRole,
) -> AppResult<Vec<Uuid>> {
    repo.ai_retrievable_ids(ws, &visible_levels(role), MAX_ALLOWLIST_IDS)
        .await
}

/// Shared by the service and the stream handler (which has no store or publisher).
pub(crate) async fn apply_index_result(
    repo: &dyn DocumentRepo,
    clock: &dyn Clock,
    ws: WorkspaceId,
    id: Uuid,
    revision: Option<i32>,
    outcome: &IndexOutcome,
) -> AppResult<bool> {
    let now = clock.now();
    let Some(doc) = repo
        .apply_index_result(ws, id, revision, outcome, now)
        .await?
    else {
        return Ok(false);
    };
    let action = match outcome {
        IndexOutcome::Indexed { .. } => "EmbeddingIndexed",
        IndexOutcome::Blocked { .. } => "EmbeddingBlocked",
        IndexOutcome::Failed { .. } => "EmbeddingFailed",
        IndexOutcome::Deleted => "EmbeddingDeleted",
    };
    let meta =
        json!({ "ingestionStatus": doc.ingestion_status, "reason": doc.ingestion_failure_reason });
    if let Err(e) = repo.audit(ws, id, None, action, meta, now).await {
        tracing::warn!(document = %id, error = %e, "document audit failed");
    }
    Ok(true)
}
