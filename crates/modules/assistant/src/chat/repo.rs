//! `assistant.chat_messages` over runtime sqlx queries. Every query filters by `workspace_id`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId};

use super::model::{Citation, HistoryItem};

#[derive(Clone)]
pub struct PgChatRepository {
    pool: PgPool,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    role: String,
    kind: String,
    content: String,
    citations: Json<Vec<Citation>>,
    model: Option<String>,
    scope: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<Row> for HistoryItem {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            role: r.role,
            kind: r.kind,
            content: r.content,
            citations: r.citations.0,
            model: r.model,
            scope: r.scope,
            created_at: r.created_at,
        }
    }
}

/// One exchange to persist.
pub struct Exchange<'a> {
    pub workspace: WorkspaceId,
    pub room: RoomId,
    pub user: UserId,
    /// `ask` | `recap`.
    pub kind: &'a str,
    pub question: &'a str,
    pub answer: &'a str,
    pub citations: &'a [Citation],
    pub model: &'a str,
    pub scope: &'a str,
    pub at: DateTime<Utc>,
}

impl PgChatRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store the user message and the answer; returns the answer's id.
    pub async fn save_exchange(&self, x: &Exchange<'_>) -> AppResult<Uuid> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let answer_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO assistant.chat_messages \
             (id, workspace_id, room_id, user_id, role, kind, content, scope, created_at) \
             VALUES ($1, $2, $3, $4, 'user', $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(x.workspace.as_uuid())
        .bind(x.room.as_uuid())
        .bind(x.user.as_uuid())
        .bind(x.kind)
        .bind(x.question)
        .bind(x.scope)
        .bind(x.at)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO assistant.chat_messages \
             (id, workspace_id, room_id, user_id, role, kind, content, citations, model, scope, created_at) \
             VALUES ($1, $2, $3, $4, 'assistant', $5, $6, $7, $8, $9, $10)",
        )
        .bind(answer_id)
        .bind(x.workspace.as_uuid())
        .bind(x.room.as_uuid())
        .bind(x.user.as_uuid())
        .bind(x.kind)
        .bind(x.answer)
        .bind(Json(x.citations))
        .bind(x.model)
        .bind(x.scope)
        .bind(x.at)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(answer_id)
    }

    /// Newest `limit` messages (older than `before` when given), returned oldest first.
    pub async fn history(
        &self,
        workspace: WorkspaceId,
        room: RoomId,
        user: UserId,
        kind: Option<&str>,
        before: Option<Uuid>,
        limit: u32,
    ) -> AppResult<Vec<HistoryItem>> {
        let mut rows: Vec<Row> = sqlx::query_as(
            "SELECT id, role, kind, content, citations, model, scope, created_at \
             FROM assistant.chat_messages \
             WHERE workspace_id = $1 AND room_id = $2 AND user_id = $3 \
               AND ($4::text IS NULL OR kind = $4) \
               AND ($5::uuid IS NULL OR id < $5) \
             ORDER BY id DESC LIMIT $6",
        )
        .bind(workspace.as_uuid())
        .bind(room.as_uuid())
        .bind(user.as_uuid())
        .bind(kind)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.reverse();
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
