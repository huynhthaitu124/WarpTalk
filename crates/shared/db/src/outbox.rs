use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::migrate::{Migration, ModuleMigrations};

/// Migrations creating `public.outbox`. Pass to `run_migrations` first.
pub const OUTBOX_MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "outbox",
    migrations: &[Migration {
        version: 1,
        name: "outbox",
        sql: include_str!("../migrations/0001_outbox.sql"),
    }],
};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OutboxRow {
    pub id: Uuid,
    pub stream: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub last_error: Option<String>,
}

/// Insert an event in the caller's transaction (atomic with the business write).
pub async fn enqueue_outbox(
    tx: &mut Transaction<'_, Postgres>,
    stream: &str,
    payload: &serde_json::Value,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO public.outbox (id, stream, payload) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(stream)
        .bind(payload)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}

/// Claim up to `limit` unpublished rows, oldest first, using `FOR UPDATE SKIP LOCKED`.
/// Increments `attempts` on the claimed rows. The row locks are held only for the
/// duration of this call's transaction (no lease): callers should publish then
/// `mark_published`, and consumers must tolerate at-least-once delivery.
pub async fn claim_outbox_batch(pool: &PgPool, limit: i64) -> Result<Vec<OutboxRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut rows: Vec<OutboxRow> = sqlx::query_as(
        "UPDATE public.outbox o SET attempts = o.attempts + 1
         FROM (
             SELECT id FROM public.outbox
             WHERE published_at IS NULL
             ORDER BY created_at, id
             LIMIT $1
             FOR UPDATE SKIP LOCKED
         ) c
         WHERE o.id = c.id
         RETURNING o.id, o.stream, o.payload, o.created_at, o.published_at, o.attempts, o.last_error",
    )
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    rows.sort_by_key(|r| (r.created_at, r.id));
    Ok(rows)
}

pub async fn mark_published(pool: &PgPool, ids: &[Uuid]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE public.outbox SET published_at = now(), last_error = NULL WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, err: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE public.outbox SET last_error = $2 WHERE id = $1")
        .bind(id)
        .bind(err)
        .execute(pool)
        .await?;
    Ok(())
}
