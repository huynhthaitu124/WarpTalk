//! sqlx implementation of [`DocumentRepo`]. Every query filters by `workspace_id`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, Page, PageRequest, UserId, WorkspaceId};

use crate::application::documents::{
    DocumentChanges, DocumentFilter, DocumentRecord, DocumentRepo, IndexOutcome, NewDocument,
};
use crate::domain::documents::{AccessLevel, ingestion};

pub struct PgDocumentRepo {
    pool: PgPool,
}

impl PgDocumentRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, name, file_name, file_extension, mime_type, size_bytes, \
    storage_provider, storage_key, document_type, access_level, is_ai_allowed, ai_eligible, \
    ingestion_status, ingestion_failure_reason, last_indexed_at, uploaded_by, revision, status, \
    created_at, updated_at";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    file_name: String,
    file_extension: String,
    mime_type: String,
    size_bytes: i64,
    storage_provider: String,
    storage_key: String,
    document_type: String,
    access_level: String,
    is_ai_allowed: bool,
    ai_eligible: bool,
    ingestion_status: String,
    ingestion_failure_reason: Option<String>,
    last_indexed_at: Option<DateTime<Utc>>,
    uploaded_by: Option<Uuid>,
    revision: i32,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
}

impl Row {
    fn into_record(self) -> AppResult<DocumentRecord> {
        Ok(DocumentRecord {
            id: self.id,
            workspace_id: WorkspaceId::from_uuid(self.workspace_id),
            title: self.name,
            file_name: self.file_name,
            file_extension: self.file_extension,
            mime_type: self.mime_type,
            size_bytes: self.size_bytes,
            storage_provider: self.storage_provider,
            storage_key: self.storage_key,
            category: self.document_type,
            access_level: AccessLevel::parse(&self.access_level)
                .map_err(|_| AppError::internal("unknown document access level"))?,
            ai_retrieval_allowed: self.is_ai_allowed,
            ai_eligible: self.ai_eligible,
            ingestion_status: self.ingestion_status,
            ingestion_failure_reason: self.ingestion_failure_reason,
            last_indexed_at: self.last_indexed_at,
            uploaded_by: self.uploaded_by.map(UserId::from_uuid),
            revision: self.revision,
            status: self.status,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn level_strings(levels: &[AccessLevel]) -> Vec<String> {
    levels.iter().map(|l| l.as_str().to_owned()).collect()
}

fn non_empty(s: &str, fallback: &str) -> String {
    if s.is_empty() { fallback } else { s }.to_owned()
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[async_trait]
impl DocumentRepo for PgDocumentRepo {
    async fn insert(&self, d: NewDocument) -> AppResult<DocumentRecord> {
        let sql = format!(
            "INSERT INTO workspace.workspace_documents \
             (id, workspace_id, uploaded_by, owner_id, name, file_name, file_extension, mime_type, \
              size_bytes, storage_provider, storage_key, source_type, document_type, ai_eligible, \
              is_ai_allowed, ingestion_status, ingestion_failure_reason, access_level, created_at, \
              updated_at) \
             VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,'upload',$11,false,$12,$13,$14,$15,$16,$16) \
             RETURNING {COLUMNS}"
        );
        let row: Row = sqlx::query_as(&sql)
            .bind(d.id)
            .bind(d.workspace_id.as_uuid())
            .bind(d.uploaded_by.as_uuid())
            .bind(&d.title)
            .bind(&d.file_name)
            .bind(&d.file_extension)
            .bind(&d.mime_type)
            .bind(d.size_bytes)
            .bind(&d.storage_provider)
            .bind(&d.storage_key)
            .bind(&d.category)
            .bind(d.ai_retrieval_allowed)
            .bind(&d.ingestion_status)
            .bind(&d.ingestion_failure_reason)
            .bind(d.access_level.as_str())
            .bind(d.now)
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        row.into_record()
    }

    async fn get(&self, ws: WorkspaceId, id: Uuid) -> AppResult<Option<DocumentRecord>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM workspace.workspace_documents \
             WHERE workspace_id = $1 AND id = $2"
        );
        sqlx::query_as::<_, Row>(&sql)
            .bind(ws.as_uuid())
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?
            .map(Row::into_record)
            .transpose()
    }

    async fn list(
        &self,
        ws: WorkspaceId,
        levels: &[AccessLevel],
        filter: &DocumentFilter,
        page: &PageRequest,
    ) -> AppResult<Page<DocumentRecord>> {
        let levels = level_strings(levels);
        let search = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", escape_like(s)));
        let push_where = |qb: &mut QueryBuilder<'_, Postgres>| {
            qb.push(" WHERE workspace_id = ")
                .push_bind(ws.as_uuid())
                .push(" AND status = 'active' AND deleted_at IS NULL AND access_level = ANY(")
                .push_bind(levels.clone())
                .push(")");
            if let Some(c) = &filter.category {
                qb.push(" AND document_type = ").push_bind(c.clone());
            }
            if let Some(s) = &search {
                qb.push(" AND name ILIKE ")
                    .push_bind(s.clone())
                    .push(" ESCAPE '\\'");
            }
        };
        let mut count = QueryBuilder::new("SELECT count(*) FROM workspace.workspace_documents");
        push_where(&mut count);
        let total: i64 = count
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_error)?;

        let mut qb = QueryBuilder::new(format!(
            "SELECT {COLUMNS} FROM workspace.workspace_documents"
        ));
        push_where(&mut qb);
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(page.limit())
            .push(" OFFSET ")
            .push_bind(page.offset());
        let rows: Vec<Row> = qb
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        let items = rows
            .into_iter()
            .map(Row::into_record)
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Page::new(items, page, total))
    }

    async fn update(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        c: &DocumentChanges,
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>> {
        let mut qb = QueryBuilder::new("UPDATE workspace.workspace_documents SET updated_at = ");
        qb.push_bind(now);
        if let Some(t) = &c.title {
            qb.push(", name = ").push_bind(t.clone());
        }
        if let Some(cat) = &c.category {
            qb.push(", document_type = ").push_bind(cat.clone());
        }
        if let Some(a) = c.access_level {
            qb.push(", access_level = ").push_bind(a.as_str());
        }
        if let Some(v) = c.ai_retrieval_allowed {
            qb.push(", is_ai_allowed = ").push_bind(v);
        }
        if let Some(v) = c.ai_eligible {
            qb.push(", ai_eligible = ").push_bind(v);
        }
        if let Some((status, reason)) = &c.ingestion {
            qb.push(", ingestion_status = ").push_bind(status.clone());
            qb.push(", ingestion_failure_reason = ")
                .push_bind(reason.clone());
        }
        if c.bump_revision {
            qb.push(", revision = revision + 1");
        }
        qb.push(" WHERE workspace_id = ")
            .push_bind(ws.as_uuid())
            .push(" AND id = ")
            .push_bind(id)
            .push(" AND status = 'active' RETURNING ")
            .push(COLUMNS);
        qb.build_query_as::<Row>()
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?
            .map(Row::into_record)
            .transpose()
    }

    async fn archive(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let r = sqlx::query(
            "UPDATE workspace.workspace_documents \
             SET status = 'archived', ai_eligible = false, deleted_at = $3, deleted_by = $4, \
                 updated_at = $3 \
             WHERE workspace_id = $1 AND id = $2 AND status = 'active'",
        )
        .bind(ws.as_uuid())
        .bind(id)
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn begin_reindex(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        statuses: &[&str],
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>> {
        let sql = format!(
            "UPDATE workspace.workspace_documents SET ingestion_status = 'pending', \
               ingestion_failure_reason = NULL, ai_eligible = false, revision = revision + 1, \
               updated_at = $3 \
             WHERE workspace_id = $1 AND id = $2 AND status = 'active' AND deleted_at IS NULL \
               AND is_ai_allowed AND ingestion_status = ANY($4) \
             RETURNING {COLUMNS}"
        );
        let statuses: Vec<String> = statuses.iter().map(|s| (*s).to_owned()).collect();
        let row: Option<Row> = sqlx::query_as(&sql)
            .bind(ws.as_uuid())
            .bind(id)
            .bind(now)
            .bind(statuses)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        row.map(Row::into_record).transpose()
    }

    async fn reindex_candidates(
        &self,
        ws: WorkspaceId,
        statuses: &[&str],
        limit: i64,
    ) -> AppResult<(Vec<Uuid>, i64)> {
        let statuses: Vec<String> = statuses.iter().map(|s| (*s).to_owned()).collect();
        let filter = "workspace_id = $1 AND status = 'active' AND deleted_at IS NULL \
                      AND is_ai_allowed AND ingestion_status = ANY($2)";
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM workspace.workspace_documents WHERE {filter}"
        ))
        .bind(ws.as_uuid())
        .bind(&statuses)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM workspace.workspace_documents WHERE {filter} \
             ORDER BY updated_at ASC NULLS FIRST, id ASC LIMIT $3"
        ))
        .bind(ws.as_uuid())
        .bind(&statuses)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok((ids, total))
    }

    async fn apply_index_result(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        revision: Option<i32>,
        outcome: &IndexOutcome,
        now: DateTime<Utc>,
    ) -> AppResult<Option<DocumentRecord>> {
        // Stale results (older revision than the row's) never apply.
        let row: Option<Row> = match outcome {
            IndexOutcome::Deleted => {
                let sql = format!(
                    "UPDATE workspace.workspace_documents SET ai_eligible = false \
                     WHERE workspace_id = $1 AND id = $2 AND ($3::int IS NULL OR revision <= $3) \
                     RETURNING {COLUMNS}"
                );
                sqlx::query_as(&sql)
                    .bind(ws.as_uuid())
                    .bind(id)
                    .bind(revision)
                    .fetch_optional(&self.pool)
                    .await
            }
            other => {
                let (status, reason, version, indexed) = match other {
                    IndexOutcome::Indexed { chunks, version } if *chunks > 0 => {
                        (ingestion::COMPLETED, None, Some(version.clone()), true)
                    }
                    // Nothing was embedded (e.g. pdf/docx waiting for an extractor).
                    IndexOutcome::Indexed { version, .. } => (
                        ingestion::SKIPPED,
                        Some("no_indexable_content".to_owned()),
                        Some(version.clone()),
                        false,
                    ),
                    IndexOutcome::Blocked { reason } => (
                        ingestion::SKIPPED,
                        Some(non_empty(reason, "embedding_blocked")),
                        None,
                        false,
                    ),
                    IndexOutcome::Failed { reason } => (
                        ingestion::FAILED,
                        Some(non_empty(reason, "embedding_failed")),
                        None,
                        false,
                    ),
                    IndexOutcome::Deleted => return Ok(None),
                };
                // Archived rows keep their status; a document whose AI use was switched off
                // meanwhile is not marked indexed.
                let sql = format!(
                    "UPDATE workspace.workspace_documents SET \
                       ingestion_status = $4, ingestion_failure_reason = $5, \
                       index_version = COALESCE($6, index_version), \
                       last_indexed_at = CASE WHEN $7 THEN $8 ELSE last_indexed_at END, \
                       ai_eligible = ($7 AND is_ai_allowed) \
                     WHERE workspace_id = $1 AND id = $2 AND status = 'active' \
                       AND ($3::int IS NULL OR revision <= $3) AND (NOT $7 OR is_ai_allowed) \
                     RETURNING {COLUMNS}"
                );
                sqlx::query_as(&sql)
                    .bind(ws.as_uuid())
                    .bind(id)
                    .bind(revision)
                    .bind(status)
                    .bind(reason)
                    .bind(version)
                    .bind(indexed)
                    .bind(now)
                    .fetch_optional(&self.pool)
                    .await
            }
        }
        .map_err(map_sqlx_error)?;
        row.map(Row::into_record).transpose()
    }

    async fn ai_retrievable_ids(
        &self,
        ws: WorkspaceId,
        levels: &[AccessLevel],
        limit: i64,
    ) -> AppResult<Vec<Uuid>> {
        let ids: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspace.workspace_documents \
             WHERE workspace_id = $1 AND status = 'active' AND deleted_at IS NULL \
               AND is_ai_allowed AND ai_eligible AND ingestion_status = 'completed' \
               AND last_indexed_at IS NOT NULL AND retention_state = 'active' \
               AND access_level = ANY($2) \
             ORDER BY updated_at DESC NULLS LAST, id DESC LIMIT $3",
        )
        .bind(ws.as_uuid())
        .bind(level_strings(levels))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(ids.into_iter().map(|(i,)| i).collect())
    }

    async fn audit(
        &self,
        ws: WorkspaceId,
        doc: Uuid,
        actor: Option<UserId>,
        action: &str,
        metadata: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO workspace.workspace_document_audits \
             (id, document_id, workspace_id, actor_id, action, action_at, metadata) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(doc)
        .bind(ws.as_uuid())
        .bind(actor.map(|a| a.as_uuid()))
        .bind(action)
        .bind(now)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }
}
