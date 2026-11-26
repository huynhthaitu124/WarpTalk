use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult};

use crate::application::ports::{
    ChunkCursor, ChunkListFilter, ChunkRecord, FactRecord, JobRecord, JobStart, JobStatus,
    KnowledgeRepository, ProfileRow, ProfileStatus, SourceKey, StoredChunk,
};
use crate::domain::{ChunkKind, EmbeddingProfile, FactCategory, SourceType};

pub struct PgKnowledgeRepository {
    pool: PgPool,
}

impl PgKnowledgeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn profile_row(r: &sqlx::postgres::PgRow) -> Result<ProfileRow, sqlx::Error> {
    Ok(ProfileRow {
        workspace_id: r.try_get("workspace_id")?,
        profile: EmbeddingProfile::new(
            r.try_get::<String, _>("provider")?,
            r.try_get::<String, _>("model")?,
            r.try_get::<i32, _>("dims")?.max(0) as usize,
            r.try_get::<String, _>("distance")?,
        ),
        collection: r.try_get("collection")?,
        status: ProfileStatus::parse(&r.try_get::<String, _>("status")?),
    })
}

const PROFILE_COLS: &str = "workspace_id, provider, model, dims, distance, collection, status";

fn job_record(r: &sqlx::postgres::PgRow) -> Result<JobRecord, sqlx::Error> {
    Ok(JobRecord {
        job_id: r.try_get("job_id")?,
        workspace_id: r.try_get("workspace_id")?,
        source_type: r.try_get("source_type")?,
        source_id: r.try_get("source_id")?,
        status: JobStatus::parse(&r.try_get::<String, _>("status")?),
        attempts: r.try_get::<i32, _>("attempts")?.max(0) as u32,
        last_error: r.try_get("last_error")?,
        chunks_indexed: r.try_get::<i32, _>("chunks_indexed")?.max(0) as u32,
        next_attempt_at: r.try_get("next_attempt_at")?,
    })
}

const CHUNK_COLS: &str =
    "id, source_type, source_id, kind, ordinal, collection, payload, indexed_at";

fn stored_chunk(r: &sqlx::postgres::PgRow) -> Result<StoredChunk, sqlx::Error> {
    let kind: String = r.try_get("kind")?;
    Ok(StoredChunk {
        id: r.try_get("id")?,
        source_type: r.try_get("source_type")?,
        source_id: r.try_get("source_id")?,
        kind: if kind == "fact" {
            ChunkKind::Fact
        } else {
            ChunkKind::Content
        },
        ordinal: r.try_get::<i32, _>("ordinal")?.max(0) as u32,
        collection: r.try_get("collection")?,
        payload: r.try_get("payload")?,
        indexed_at: r.try_get("indexed_at")?,
    })
}

fn fact_record(r: &sqlx::postgres::PgRow) -> Result<FactRecord, sqlx::Error> {
    let source_type: String = r.try_get("source_type")?;
    let category: String = r.try_get("category")?;
    Ok(FactRecord {
        id: r.try_get("id")?,
        workspace_id: r.try_get("workspace_id")?,
        source_type: source_type
            .parse()
            .map_err(|e: String| sqlx::Error::Decode(e.into()))?,
        source_id: r.try_get("source_id")?,
        category: FactCategory::parse(&category)
            .ok_or_else(|| sqlx::Error::Decode(format!("bad category {category}").into()))?,
        fact: r.try_get("fact")?,
        quote: r.try_get("quote")?,
        confidence: r.try_get("confidence")?,
        source_chunk_ids: r.try_get("source_chunk_ids")?,
        chunk_id: r.try_get("chunk_id")?,
    })
}

#[async_trait]
impl KnowledgeRepository for PgKnowledgeRepository {
    async fn active_profile(&self, workspace: Uuid) -> AppResult<Option<ProfileRow>> {
        let row = sqlx::query(&format!(
            "SELECT {PROFILE_COLS} FROM knowledge.embedding_profiles
              WHERE workspace_id = $1 AND status = 'active'"
        ))
        .bind(workspace)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(profile_row)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn register_profile(
        &self,
        workspace: Uuid,
        desired: &EmbeddingProfile,
    ) -> AppResult<ProfileRow> {
        let hash = desired.hash();
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("knowledge:profile:{workspace}"))
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let existing = sqlx::query(&format!(
            "SELECT {PROFILE_COLS} FROM knowledge.embedding_profiles
              WHERE workspace_id = $1 AND profile_hash = $2"
        ))
        .bind(workspace)
        .bind(&hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        if let Some(r) = existing {
            tx.commit().await.map_err(map_sqlx_error)?;
            return profile_row(&r).map_err(map_sqlx_error);
        }
        let has_active: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM knowledge.embedding_profiles
                             WHERE workspace_id = $1 AND status = 'active')",
        )
        .bind(workspace)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let status = if has_active {
            ProfileStatus::Building
        } else {
            ProfileStatus::Active
        };
        let collection = desired.collection(workspace);
        sqlx::query(
            "INSERT INTO knowledge.embedding_profiles
                (workspace_id, profile_hash, provider, model, dims, distance, collection, status,
                 activated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                     CASE WHEN $8 = 'active' THEN now() END)",
        )
        .bind(workspace)
        .bind(&hash)
        .bind(&desired.provider)
        .bind(&desired.model)
        .bind(desired.dims as i32)
        .bind(&desired.distance)
        .bind(&collection)
        .bind(status.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(ProfileRow {
            workspace_id: workspace,
            profile: desired.clone(),
            collection,
            status,
        })
    }

    async fn activate_profile(&self, workspace: Uuid, profile_hash: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("knowledge:profile:{workspace}"))
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM knowledge.embedding_profiles
                             WHERE workspace_id = $1 AND profile_hash = $2)",
        )
        .bind(workspace)
        .bind(profile_hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        if !exists {
            return Err(AppError::not_found("embedding profile not found"));
        }
        sqlx::query(
            "UPDATE knowledge.embedding_profiles SET status = 'retired'
              WHERE workspace_id = $1 AND status = 'active' AND profile_hash <> $2",
        )
        .bind(workspace)
        .bind(profile_hash)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE knowledge.embedding_profiles SET status = 'active', activated_at = now()
              WHERE workspace_id = $1 AND profile_hash = $2 AND status <> 'active'",
        )
        .bind(workspace)
        .bind(profile_hash)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn profiles(&self, workspace: Uuid) -> AppResult<Vec<ProfileRow>> {
        let rows = sqlx::query(&format!(
            "SELECT {PROFILE_COLS} FROM knowledge.embedding_profiles
              WHERE workspace_id = $1 ORDER BY created_at"
        ))
        .bind(workspace)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(profile_row)
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)
    }

    async fn begin_job(
        &self,
        workspace: Uuid,
        job_id: &str,
        source_type: SourceType,
        source_id: &str,
        message_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<JobStart> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let existing = sqlx::query(
            "SELECT status, attempts, last_message_id, next_attempt_at
               FROM knowledge.index_jobs WHERE workspace_id = $1 AND job_id = $2 FOR UPDATE",
        )
        .bind(workspace)
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let attempts = match existing {
            Some(r) => {
                let status: String = r.try_get("status").map_err(map_sqlx_error)?;
                let prev: i32 = r.try_get("attempts").map_err(map_sqlx_error)?;
                let last_msg: Option<String> =
                    r.try_get("last_message_id").map_err(map_sqlx_error)?;
                let next: Option<DateTime<Utc>> =
                    r.try_get("next_attempt_at").map_err(map_sqlx_error)?;
                // Same stream entry redelivered after a failed attempt: continue the cycle.
                let continuing = matches!(status.as_str(), "retrying" | "running")
                    && last_msg.as_deref() == Some(message_id);
                if continuing {
                    if let Some(until) = next.filter(|t| *t > now) {
                        tx.commit().await.map_err(map_sqlx_error)?;
                        return Ok(JobStart::Backoff { until });
                    }
                    prev.max(0) as u32 + 1
                } else {
                    1
                }
            }
            None => 1,
        };
        sqlx::query(
            "INSERT INTO knowledge.index_jobs
                (job_id, workspace_id, source_type, source_id, status, attempts, last_message_id,
                 next_attempt_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'running', $5, $6, NULL, $7, $7)
             ON CONFLICT (workspace_id, job_id) DO UPDATE
                SET status = 'running', attempts = $5, last_message_id = $6,
                    source_type = $3, source_id = $4, next_attempt_at = NULL,
                    completed_at = NULL, updated_at = $7,
                    last_error = CASE WHEN $5 = 1 THEN NULL ELSE knowledge.index_jobs.last_error END",
        )
        .bind(job_id)
        .bind(workspace)
        .bind(source_type.as_str())
        .bind(source_id)
        .bind(attempts as i32)
        .bind(message_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(JobStart::Started { attempts })
    }

    async fn fail_attempt(
        &self,
        workspace: Uuid,
        job_id: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE knowledge.index_jobs
                SET status = 'retrying', last_error = $3, next_attempt_at = $4, updated_at = now()
              WHERE workspace_id = $1 AND job_id = $2",
        )
        .bind(workspace)
        .bind(job_id)
        .bind(error)
        .bind(next_attempt_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn finish_job(
        &self,
        workspace: Uuid,
        job_id: &str,
        status: JobStatus,
        chunks_indexed: u32,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE knowledge.index_jobs
                SET status = $3, chunks_indexed = $4, last_error = COALESCE($5, last_error),
                    next_attempt_at = NULL, completed_at = $6, updated_at = $6
              WHERE workspace_id = $1 AND job_id = $2",
        )
        .bind(workspace)
        .bind(job_id)
        .bind(status.as_str())
        .bind(chunks_indexed as i32)
        .bind(error)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn job(&self, workspace: Uuid, job_id: &str) -> AppResult<Option<JobRecord>> {
        let row = sqlx::query(
            "SELECT job_id, workspace_id, source_type, source_id, status, attempts, last_error,
                    chunks_indexed, next_attempt_at
               FROM knowledge.index_jobs WHERE workspace_id = $1 AND job_id = $2",
        )
        .bind(workspace)
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(job_record)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn replace_chunks(
        &self,
        src: SourceKey<'_>,
        kind: ChunkKind,
        collection: &str,
        records: &[ChunkRecord],
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        for r in records {
            sqlx::query(
                "INSERT INTO knowledge.chunks
                    (id, workspace_id, source_type, source_id, kind, ordinal, text_hash,
                     collection, payload, indexed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (workspace_id, id) DO UPDATE
                    SET source_type = $3, source_id = $4, kind = $5, ordinal = $6, text_hash = $7,
                        collection = $8, payload = $9, indexed_at = $10",
            )
            .bind(r.id)
            .bind(src.workspace)
            .bind(src.source_type.as_str())
            .bind(src.source_id)
            .bind(r.kind.as_str())
            .bind(r.ordinal as i32)
            .bind(&r.text_hash)
            .bind(collection)
            .bind(&r.payload)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        }
        let keep: Vec<Uuid> = records.iter().map(|r| r.id).collect();
        sqlx::query(
            "DELETE FROM knowledge.chunks
              WHERE workspace_id = $1 AND source_type = $2 AND source_id = $3 AND kind = $4
                AND id <> ALL($5)",
        )
        .bind(src.workspace)
        .bind(src.source_type.as_str())
        .bind(src.source_id)
        .bind(kind.as_str())
        .bind(&keep)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn count_chunks(&self, src: SourceKey<'_>) -> AppResult<u64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM knowledge.chunks
              WHERE workspace_id = $1 AND source_type = $2 AND source_id = $3",
        )
        .bind(src.workspace)
        .bind(src.source_type.as_str())
        .bind(src.source_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(n.max(0) as u64)
    }

    async fn replace_facts(
        &self,
        src: SourceKey<'_>,
        facts: &[FactRecord],
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query(
            "DELETE FROM knowledge.facts
              WHERE workspace_id = $1 AND source_type = $2 AND source_id = $3",
        )
        .bind(src.workspace)
        .bind(src.source_type.as_str())
        .bind(src.source_id)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        for f in facts {
            sqlx::query(
                "INSERT INTO knowledge.facts
                    (id, workspace_id, source_type, source_id, category, fact, quote, confidence,
                     source_chunk_ids, chunk_id, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(f.id)
            .bind(src.workspace)
            .bind(src.source_type.as_str())
            .bind(src.source_id)
            .bind(f.category.as_str())
            .bind(&f.fact)
            .bind(&f.quote)
            .bind(f.confidence)
            .bind(&f.source_chunk_ids)
            .bind(f.chunk_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        }
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn list_facts(
        &self,
        workspace: Uuid,
        source_type: Option<SourceType>,
        category: Option<FactCategory>,
        limit: u32,
    ) -> AppResult<Vec<FactRecord>> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, source_type, source_id, category, fact, quote, confidence,
                    source_chunk_ids, chunk_id
               FROM knowledge.facts
              WHERE workspace_id = $1
                AND ($2::text IS NULL OR source_type = $2)
                AND ($3::text IS NULL OR category = $3)
              ORDER BY created_at DESC, id DESC
              LIMIT $4",
        )
        .bind(workspace)
        .bind(source_type.map(SourceType::as_str))
        .bind(category.map(FactCategory::as_str))
        .bind(i64::from(limit.min(500)))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(fact_record)
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)
    }

    async fn list_chunks(
        &self,
        workspace: Uuid,
        filter: &ChunkListFilter,
        after: Option<ChunkCursor>,
        limit: u32,
    ) -> AppResult<Vec<StoredChunk>> {
        let docs = filter
            .allowed_document_ids
            .as_ref()
            .map(|v| v.iter().map(Uuid::to_string).collect::<Vec<_>>());
        let meetings = filter
            .allowed_meeting_ids
            .as_ref()
            .map(|v| v.iter().map(Uuid::to_string).collect::<Vec<_>>());
        let rows = sqlx::query(&format!(
            "SELECT {CHUNK_COLS} FROM knowledge.chunks
              WHERE workspace_id = $1
                AND source_type <> 'transcript'
                AND ($2::text[] IS NULL OR source_type = ANY($2))
                AND ($3::text IS NULL OR (kind = 'fact' AND payload->>'fact_category' = $3))
                AND ($4::text[] IS NULL OR source_type <> 'document' OR source_id = ANY($4))
                AND ($5::text[] IS NULL OR source_type <> 'meeting_summary'
                     OR payload->>'meeting_id' = ANY($5))
                AND ($6::timestamptz IS NULL OR (indexed_at, id) < ($6, $7::uuid))
              ORDER BY indexed_at DESC, id DESC
              LIMIT $8"
        ))
        .bind(workspace)
        .bind(&filter.source_types)
        .bind(filter.fact_category.map(FactCategory::as_str))
        .bind(docs)
        .bind(meetings)
        .bind(after.map(|c| c.indexed_at))
        .bind(after.map(|c| c.id))
        .bind(i64::from(limit.min(1000)))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(stored_chunk)
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)
    }

    async fn get_chunk(&self, workspace: Uuid, id: Uuid) -> AppResult<Option<StoredChunk>> {
        let row = sqlx::query(&format!(
            "SELECT {CHUNK_COLS} FROM knowledge.chunks
              WHERE workspace_id = $1 AND id = $2 AND source_type <> 'transcript'"
        ))
        .bind(workspace)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(stored_chunk)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn update_fact_chunk(
        &self,
        workspace: Uuid,
        id: Uuid,
        category: FactCategory,
        fact: &str,
        payload: &serde_json::Value,
        text_hash: &str,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let n = sqlx::query(
            "UPDATE knowledge.chunks SET payload = $3, text_hash = $4
              WHERE workspace_id = $1 AND id = $2 AND kind = 'fact'",
        )
        .bind(workspace)
        .bind(id)
        .bind(payload)
        .bind(text_hash)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("knowledge chunk not found"));
        }
        sqlx::query(
            "UPDATE knowledge.facts SET fact = $3, category = $4
              WHERE workspace_id = $1 AND chunk_id = $2",
        )
        .bind(workspace)
        .bind(id)
        .bind(fact)
        .bind(category.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn delete_chunk(&self, workspace: Uuid, id: Uuid) -> AppResult<bool> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query("DELETE FROM knowledge.facts WHERE workspace_id = $1 AND chunk_id = $2")
            .bind(workspace)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let n = sqlx::query("DELETE FROM knowledge.chunks WHERE workspace_id = $1 AND id = $2")
            .bind(workspace)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(n > 0)
    }

    async fn purge_rows(
        &self,
        workspace: Uuid,
        source_type: Option<SourceType>,
        source_id: Option<&str>,
    ) -> AppResult<(u64, u64)> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let chunks = sqlx::query(
            "DELETE FROM knowledge.chunks
              WHERE workspace_id = $1
                AND ($2::text IS NULL OR source_type = $2)
                AND ($3::text IS NULL OR source_id = $3)",
        )
        .bind(workspace)
        .bind(source_type.map(SourceType::as_str))
        .bind(source_id)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        let facts = sqlx::query(
            "DELETE FROM knowledge.facts
              WHERE workspace_id = $1
                AND ($2::text IS NULL OR source_type = $2)
                AND ($3::text IS NULL OR source_id = $3)",
        )
        .bind(workspace)
        .bind(source_type.map(SourceType::as_str))
        .bind(source_id)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok((chunks, facts))
    }
}
