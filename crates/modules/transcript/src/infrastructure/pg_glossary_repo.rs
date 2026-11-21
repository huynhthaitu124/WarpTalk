//! sqlx adapter for `GlossaryRepo` (runtime queries; schema `transcript`).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};

use crate::application::glossary_ports::{
    GlobalAuditEntry, GlossaryPatch, GlossaryRepo, ImportOutcome, NewGlossary, PublishOutcome,
    TermFields,
};
use crate::domain::glossary::{
    GlobalAudit, GlobalStatus, GlobalTerm, GlobalTermFields, Glossary, GlossaryTerm,
    MAX_PUBLISHED_GLOBAL_TERMS, SnapshotTerm,
};

pub struct PgGlossaryRepo {
    pool: PgPool,
}

impl PgGlossaryRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const GLOSSARY_COLS: &str = "id, workspace_id, name, description, source_language, \
    target_language, term_count, created_at, updated_at";
const TERM_COLS: &str = "id, glossary_id, source_term, target_term, case_sensitive, \
    usage_note, created_at";
const GLOBAL_COLS: &str = "id, term, preferred_translation, source_language, target_language, \
    business_domain, definition, usage_note, priority, status, version, created_at, updated_at";

/// Serializes publishes so the published cap cannot be exceeded by concurrent requests.
const PUBLISH_LOCK_KEY: i64 = 7_021_001;

fn glossary_from(r: &PgRow) -> AppResult<Glossary> {
    let g = map_sqlx_error;
    Ok(Glossary {
        id: r.try_get("id").map_err(g)?,
        workspace_id: WorkspaceId::from_uuid(r.try_get("workspace_id").map_err(g)?),
        name: r.try_get("name").map_err(g)?,
        description: r.try_get("description").map_err(g)?,
        source_language: r.try_get("source_language").map_err(g)?,
        target_language: r.try_get("target_language").map_err(g)?,
        term_count: r.try_get("term_count").map_err(g)?,
        created_at: r.try_get("created_at").map_err(g)?,
        updated_at: r.try_get("updated_at").map_err(g)?,
    })
}

fn term_from(r: &PgRow) -> AppResult<GlossaryTerm> {
    let g = map_sqlx_error;
    Ok(GlossaryTerm {
        id: r.try_get("id").map_err(g)?,
        glossary_id: r.try_get("glossary_id").map_err(g)?,
        source_term: r.try_get("source_term").map_err(g)?,
        target_term: r.try_get("target_term").map_err(g)?,
        case_sensitive: r.try_get("case_sensitive").map_err(g)?,
        notes: r.try_get("usage_note").map_err(g)?,
        created_at: r.try_get("created_at").map_err(g)?,
    })
}

fn global_from(r: &PgRow) -> AppResult<GlobalTerm> {
    let g = map_sqlx_error;
    let status: String = r.try_get("status").map_err(g)?;
    Ok(GlobalTerm {
        id: r.try_get("id").map_err(g)?,
        term: r.try_get("term").map_err(g)?,
        preferred_translation: r.try_get("preferred_translation").map_err(g)?,
        source_language: r.try_get("source_language").map_err(g)?,
        target_language: r.try_get("target_language").map_err(g)?,
        business_domain: r.try_get("business_domain").map_err(g)?,
        definition: r.try_get("definition").map_err(g)?,
        usage_note: r.try_get("usage_note").map_err(g)?,
        priority: r.try_get("priority").map_err(g)?,
        status: GlobalStatus::parse(&status)
            .ok_or_else(|| AppError::internal(format!("unknown glossary status `{status}`")))?,
        version: r.try_get("version").map_err(g)?,
        created_at: r.try_get("created_at").map_err(g)?,
        updated_at: r.try_get("updated_at").map_err(g)?,
    })
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn dup_term(e: sqlx::Error) -> AppError {
    if is_unique_violation(&e) {
        AppError::conflict("DUPLICATE_TERM", "term already exists in this glossary")
    } else {
        map_sqlx_error(e)
    }
}

fn dup_global(e: sqlx::Error) -> AppError {
    if is_unique_violation(&e) {
        AppError::validation(
            "DUPLICATE_TERM",
            "a global term with this term, languages and domain already exists",
        )
    } else {
        map_sqlx_error(e)
    }
}

async fn recount(tx: &mut Transaction<'_, Postgres>, glossary: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE transcript.glossaries SET term_count = (SELECT count(*)::int FROM \
         transcript.glossary_terms WHERE glossary_id = $1 AND deleted_at IS NULL) WHERE id = $1",
    )
    .bind(glossary)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    term: Uuid,
    entry: &GlobalAuditEntry,
    before: Option<Value>,
    after: Option<Value>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO transcript.global_glossary_audits \
         (id, term_id, action, before_json, after_json, actor_user_id, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(term)
    .bind(entry.action)
    .bind(before)
    .bind(after)
    .bind(entry.actor.as_uuid())
    .bind(entry.now)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn lock_global(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> AppResult<Option<GlobalTerm>> {
    sqlx::query(&format!(
        "SELECT {GLOBAL_COLS} FROM transcript.global_glossary_terms \
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_sqlx_error)?
    .as_ref()
    .map(global_from)
    .transpose()
}

fn snapshot_from(r: &PgRow) -> AppResult<SnapshotTerm> {
    let g = map_sqlx_error;
    Ok(SnapshotTerm {
        source: r.try_get("source").map_err(g)?,
        target: r.try_get("target").map_err(g)?,
        source_lang: r.try_get("source_lang").map_err(g)?,
        target_lang: r.try_get("target_lang").map_err(g)?,
        case_sensitive: r.try_get("case_sensitive").map_err(g)?,
    })
}

#[async_trait]
impl GlossaryRepo for PgGlossaryRepo {
    async fn create_glossary(&self, g: &NewGlossary) -> AppResult<Glossary> {
        let row = sqlx::query(&format!(
            "INSERT INTO transcript.glossaries (id, workspace_id, name, description, \
             source_language, target_language, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {GLOSSARY_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(g.workspace_id.as_uuid())
        .bind(&g.name)
        .bind(&g.description)
        .bind(&g.source_language)
        .bind(&g.target_language)
        .bind(g.now)
        .bind(g.actor.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        glossary_from(&row)
    }

    async fn glossary(&self, id: Uuid) -> AppResult<Option<Glossary>> {
        sqlx::query(&format!(
            "SELECT {GLOSSARY_COLS} FROM transcript.glossaries WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .as_ref()
        .map(glossary_from)
        .transpose()
    }

    async fn glossaries_by_workspace(&self, ws: WorkspaceId) -> AppResult<Vec<Glossary>> {
        sqlx::query(&format!(
            "SELECT {GLOSSARY_COLS} FROM transcript.glossaries \
             WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY lower(name), id"
        ))
        .bind(ws.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .iter()
        .map(glossary_from)
        .collect()
    }

    async fn update_glossary(
        &self,
        id: Uuid,
        p: &GlossaryPatch,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Option<Glossary>> {
        let (set_desc, desc) = match &p.description {
            Some(d) => (true, d.clone()),
            None => (false, None),
        };
        sqlx::query(&format!(
            "UPDATE transcript.glossaries SET name = COALESCE($2, name), \
             description = CASE WHEN $3 THEN $4 ELSE description END, \
             source_language = COALESCE($5, source_language), \
             target_language = COALESCE($6, target_language), \
             updated_at = $7, updated_by = $8 \
             WHERE id = $1 AND deleted_at IS NULL RETURNING {GLOSSARY_COLS}"
        ))
        .bind(id)
        .bind(&p.name)
        .bind(set_desc)
        .bind(desc)
        .bind(&p.source_language)
        .bind(&p.target_language)
        .bind(now)
        .bind(actor.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .as_ref()
        .map(glossary_from)
        .transpose()
    }

    async fn delete_glossary(
        &self,
        id: Uuid,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let r = sqlx::query(
            "UPDATE transcript.glossaries SET deleted_at = $2, deleted_by = $3, is_active = false \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn terms(&self, glossary: Uuid) -> AppResult<Vec<GlossaryTerm>> {
        sqlx::query(&format!(
            "SELECT {TERM_COLS} FROM transcript.glossary_terms \
             WHERE glossary_id = $1 AND deleted_at IS NULL ORDER BY lower(source_term), id"
        ))
        .bind(glossary)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .iter()
        .map(term_from)
        .collect()
    }

    async fn term(&self, glossary: Uuid, term: Uuid) -> AppResult<Option<GlossaryTerm>> {
        sqlx::query(&format!(
            "SELECT {TERM_COLS} FROM transcript.glossary_terms \
             WHERE id = $1 AND glossary_id = $2 AND deleted_at IS NULL"
        ))
        .bind(term)
        .bind(glossary)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .as_ref()
        .map(term_from)
        .transpose()
    }

    async fn add_term(
        &self,
        glossary: Uuid,
        t: &TermFields,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<GlossaryTerm> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let row = sqlx::query(&format!(
            "INSERT INTO transcript.glossary_terms (id, glossary_id, source_term, target_term, \
             case_sensitive, usage_note, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {TERM_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(glossary)
        .bind(&t.source_term)
        .bind(&t.target_term)
        .bind(t.case_sensitive)
        .bind(&t.notes)
        .bind(now)
        .bind(actor.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(dup_term)?;
        recount(&mut tx, glossary).await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        term_from(&row)
    }

    async fn update_term(
        &self,
        glossary: Uuid,
        term: Uuid,
        t: &TermFields,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Option<GlossaryTerm>> {
        sqlx::query(&format!(
            "UPDATE transcript.glossary_terms SET source_term = $3, target_term = $4, \
             case_sensitive = $5, usage_note = $6, updated_at = $7, updated_by = $8 \
             WHERE id = $1 AND glossary_id = $2 AND deleted_at IS NULL RETURNING {TERM_COLS}"
        ))
        .bind(term)
        .bind(glossary)
        .bind(&t.source_term)
        .bind(&t.target_term)
        .bind(t.case_sensitive)
        .bind(&t.notes)
        .bind(now)
        .bind(actor.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(dup_term)?
        .as_ref()
        .map(term_from)
        .transpose()
    }

    async fn delete_term(
        &self,
        glossary: Uuid,
        term: Uuid,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let r = sqlx::query(
            "UPDATE transcript.glossary_terms SET deleted_at = $3, deleted_by = $4, is_active = false \
             WHERE id = $1 AND glossary_id = $2 AND deleted_at IS NULL",
        )
        .bind(term)
        .bind(glossary)
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        recount(&mut tx, glossary).await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn import_terms(
        &self,
        glossary: Uuid,
        pairs: &[(String, String)],
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<ImportOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query("SELECT id FROM transcript.glossaries WHERE id = $1 FOR UPDATE")
            .bind(glossary)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let ids: Vec<Uuid> = pairs.iter().map(|_| Uuid::now_v7()).collect();
        let sources: Vec<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
        let targets: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        let r = sqlx::query(
            "INSERT INTO transcript.glossary_terms (id, glossary_id, source_term, target_term, \
             created_at, created_by) \
             SELECT u.id, $1, u.s, u.t, $5, $6 FROM UNNEST($2::uuid[], $3::text[], $4::text[]) \
             AS u(id, s, t) \
             ON CONFLICT (glossary_id, lower(source_term)) WHERE deleted_at IS NULL DO NOTHING",
        )
        .bind(glossary)
        .bind(&ids)
        .bind(&sources)
        .bind(&targets)
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        recount(&mut tx, glossary).await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        let imported = usize::try_from(r.rows_affected()).unwrap_or(0);
        Ok(ImportOutcome {
            imported,
            skipped_existing: pairs.len().saturating_sub(imported),
        })
    }

    async fn workspace_snapshot(&self, ws: WorkspaceId) -> AppResult<Vec<SnapshotTerm>> {
        sqlx::query(
            "SELECT t.source_term AS source, t.target_term AS target, t.case_sensitive, \
             g.source_language AS source_lang, g.target_language AS target_lang \
             FROM transcript.glossary_terms t JOIN transcript.glossaries g ON g.id = t.glossary_id \
             WHERE g.workspace_id = $1 AND g.deleted_at IS NULL AND g.is_active \
             AND t.deleted_at IS NULL AND t.is_active \
             ORDER BY g.created_at, lower(t.source_term), t.id",
        )
        .bind(ws.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .iter()
        .map(snapshot_from)
        .collect()
    }

    async fn global_terms(
        &self,
        status: Option<GlobalStatus>,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<GlobalTerm>, i64)> {
        let st = status.map(GlobalStatus::as_str);
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transcript.global_glossary_terms \
             WHERE deleted_at IS NULL AND ($1::text IS NULL OR status = $1)",
        )
        .bind(st)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let items = sqlx::query(&format!(
            "SELECT {GLOBAL_COLS} FROM transcript.global_glossary_terms \
             WHERE deleted_at IS NULL AND ($1::text IS NULL OR status = $1) \
             ORDER BY lower(term), created_at, id LIMIT $2 OFFSET $3"
        ))
        .bind(st)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .iter()
        .map(global_from)
        .collect::<AppResult<Vec<_>>>()?;
        Ok((items, total))
    }

    async fn global_term(&self, id: Uuid) -> AppResult<Option<GlobalTerm>> {
        sqlx::query(&format!(
            "SELECT {GLOBAL_COLS} FROM transcript.global_glossary_terms \
             WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .as_ref()
        .map(global_from)
        .transpose()
    }

    async fn create_global(
        &self,
        id: Uuid,
        f: &GlobalTermFields,
        entry: &GlobalAuditEntry,
    ) -> AppResult<GlobalTerm> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let row = sqlx::query(&format!(
            "INSERT INTO transcript.global_glossary_terms (id, term, preferred_translation, \
             source_language, target_language, business_domain, definition, usage_note, \
             priority, status, version, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft', 1, $10, $11) \
             RETURNING {GLOBAL_COLS}"
        ))
        .bind(id)
        .bind(&f.term)
        .bind(&f.preferred_translation)
        .bind(&f.source_language)
        .bind(&f.target_language)
        .bind(&f.business_domain)
        .bind(&f.definition)
        .bind(&f.usage_note)
        .bind(f.priority)
        .bind(entry.now)
        .bind(entry.actor.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(dup_global)?;
        let t = global_from(&row)?;
        audit(&mut tx, id, entry, None, Some(t.audit_json())).await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(t)
    }

    async fn update_global(
        &self,
        id: Uuid,
        f: &GlobalTermFields,
        entry: &GlobalAuditEntry,
    ) -> AppResult<Option<GlobalTerm>> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let Some(before) = lock_global(&mut tx, id).await? else {
            return Ok(None);
        };
        let row = sqlx::query(&format!(
            "UPDATE transcript.global_glossary_terms SET term = $2, preferred_translation = $3, \
             source_language = $4, target_language = $5, business_domain = $6, definition = $7, \
             usage_note = $8, priority = $9, version = version + 1, updated_at = $10, \
             updated_by = $11 WHERE id = $1 RETURNING {GLOBAL_COLS}"
        ))
        .bind(id)
        .bind(&f.term)
        .bind(&f.preferred_translation)
        .bind(&f.source_language)
        .bind(&f.target_language)
        .bind(&f.business_domain)
        .bind(&f.definition)
        .bind(&f.usage_note)
        .bind(f.priority)
        .bind(entry.now)
        .bind(entry.actor.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(dup_global)?;
        let after = global_from(&row)?;
        audit(
            &mut tx,
            id,
            entry,
            Some(before.audit_json()),
            Some(after.audit_json()),
        )
        .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(Some(after))
    }

    async fn publish_global(
        &self,
        id: Uuid,
        entry: &GlobalAuditEntry,
    ) -> AppResult<PublishOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(PUBLISH_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let Some(before) = lock_global(&mut tx, id).await? else {
            return Ok(PublishOutcome::NotFound);
        };
        let published: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transcript.global_glossary_terms \
             WHERE status = 'published' AND deleted_at IS NULL",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        if before.status != GlobalStatus::Published && published >= MAX_PUBLISHED_GLOBAL_TERMS {
            return Ok(PublishOutcome::CapReached);
        }
        let row = sqlx::query(&format!(
            "UPDATE transcript.global_glossary_terms SET status = 'published', \
             version = version + 1, updated_at = $2, updated_by = $3 WHERE id = $1 \
             RETURNING {GLOBAL_COLS}"
        ))
        .bind(id)
        .bind(entry.now)
        .bind(entry.actor.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let after = global_from(&row)?;
        audit(
            &mut tx,
            id,
            entry,
            Some(before.audit_json()),
            Some(after.audit_json()),
        )
        .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(PublishOutcome::Published(Box::new(after)))
    }

    async fn archive_global(
        &self,
        id: Uuid,
        entry: &GlobalAuditEntry,
    ) -> AppResult<Option<GlobalTerm>> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let Some(before) = lock_global(&mut tx, id).await? else {
            return Ok(None);
        };
        let row = sqlx::query(&format!(
            "UPDATE transcript.global_glossary_terms SET status = 'archived', \
             version = version + 1, updated_at = $2, updated_by = $3 WHERE id = $1 \
             RETURNING {GLOBAL_COLS}"
        ))
        .bind(id)
        .bind(entry.now)
        .bind(entry.actor.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let after = global_from(&row)?;
        audit(
            &mut tx,
            id,
            entry,
            Some(before.audit_json()),
            Some(after.audit_json()),
        )
        .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(Some(after))
    }

    async fn delete_global(&self, id: Uuid, entry: &GlobalAuditEntry) -> AppResult<bool> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let Some(before) = lock_global(&mut tx, id).await? else {
            return Ok(false);
        };
        sqlx::query("DELETE FROM transcript.global_glossary_terms WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        audit(&mut tx, id, entry, Some(before.audit_json()), None).await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(true)
    }

    async fn global_audits(&self, term: Uuid) -> AppResult<Vec<GlobalAudit>> {
        let rows = sqlx::query(
            "SELECT id, term_id, action, before_json, after_json, actor_user_id, created_at \
             FROM transcript.global_glossary_audits WHERE term_id = $1 ORDER BY created_at, id",
        )
        .bind(term)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let g = map_sqlx_error;
        rows.iter()
            .map(|r| {
                Ok(GlobalAudit {
                    id: r.try_get("id").map_err(g)?,
                    term_id: r.try_get("term_id").map_err(g)?,
                    action: r.try_get("action").map_err(g)?,
                    before_json: r.try_get("before_json").map_err(g)?,
                    after_json: r.try_get("after_json").map_err(g)?,
                    actor_user_id: r.try_get("actor_user_id").map_err(g)?,
                    created_at: r.try_get("created_at").map_err(g)?,
                })
            })
            .collect()
    }

    async fn published_global_snapshot(&self) -> AppResult<Vec<SnapshotTerm>> {
        sqlx::query(
            "SELECT term AS source, preferred_translation AS target, false AS case_sensitive, \
             source_language AS source_lang, target_language AS target_lang \
             FROM transcript.global_glossary_terms \
             WHERE status = 'published' AND deleted_at IS NULL ORDER BY lower(term), id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .iter()
        .map(snapshot_from)
        .collect()
    }
}
