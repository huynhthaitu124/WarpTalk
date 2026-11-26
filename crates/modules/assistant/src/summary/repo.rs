//! `assistant.meeting_summaries` and `assistant.action_items` over runtime sqlx queries.
//! Every action item query filters by `workspace_id`.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, RoomId, WorkspaceId};

use super::model::{
    ActionItem, ActionItemDraft, SummaryContent, SummaryJob, SummaryStatus, Trigger,
};

const JOB_COLS: &str = "id, workspace_id, room_id, version, transcript_version, trigger, status, \
     model, prompt_version, content, error_code, error, attempts, requested_by, input_segments, \
     tokens_in, tokens_out, created_at, started_at, completed_at, updated_at";

#[derive(sqlx::FromRow)]
struct JobRow {
    id: Uuid,
    workspace_id: Uuid,
    room_id: Uuid,
    version: i32,
    transcript_version: i32,
    trigger: String,
    status: String,
    model: Option<String>,
    prompt_version: Option<String>,
    content: Option<Json<SummaryContent>>,
    error_code: Option<String>,
    error: Option<String>,
    attempts: i32,
    requested_by: Option<Uuid>,
    input_segments: Option<i32>,
    tokens_in: i64,
    tokens_out: i64,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<JobRow> for SummaryJob {
    type Error = AppError;
    fn try_from(r: JobRow) -> AppResult<Self> {
        Ok(Self {
            id: r.id,
            workspace_id: WorkspaceId::from_uuid(r.workspace_id),
            room_id: RoomId::from_uuid(r.room_id),
            version: r.version,
            transcript_version: r.transcript_version,
            trigger: r.trigger,
            status: SummaryStatus::parse(&r.status).ok_or_else(|| {
                AppError::internal(format!("unexpected summary status `{}`", r.status))
            })?,
            model: r.model,
            prompt_version: r.prompt_version,
            content: r.content.map(|j| j.0),
            error_code: r.error_code,
            error: r.error,
            attempts: r.attempts,
            requested_by: r.requested_by,
            input_segments: r.input_segments,
            tokens_in: r.tokens_in,
            tokens_out: r.tokens_out,
            created_at: r.created_at,
            started_at: r.started_at,
            completed_at: r.completed_at,
            updated_at: r.updated_at,
        })
    }
}

const ITEM_COLS: &str = "id, workspace_id, room_id, summary_id, task, owner, due, status, \
     source_segment_ids, assignee_user_id, created_at, updated_at";

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: Uuid,
    workspace_id: Uuid,
    room_id: Uuid,
    summary_id: Uuid,
    task: String,
    owner: Option<String>,
    due: Option<String>,
    status: String,
    source_segment_ids: Vec<Uuid>,
    assignee_user_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ItemRow> for ActionItem {
    fn from(r: ItemRow) -> Self {
        Self {
            id: r.id,
            workspace_id: WorkspaceId::from_uuid(r.workspace_id),
            room_id: RoomId::from_uuid(r.room_id),
            summary_id: r.summary_id,
            task: r.task,
            owner: r.owner,
            due: r.due,
            status: r.status,
            source_segment_ids: r.source_segment_ids,
            assignee_user_id: r.assignee_user_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Result of enqueueing a job.
#[derive(Debug)]
pub enum Enqueued {
    Created(SummaryJob),
    /// An automatic job for this room and transcript version already exists.
    Existing(SummaryJob),
}

/// Edit of an action item (`None` = leave, `Some(None)` = clear).
#[derive(Debug, Clone, Default)]
pub struct ItemPatch {
    pub task: Option<String>,
    pub owner: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub status: Option<String>,
}

/// Filter of the workspace-wide list.
#[derive(Debug, Clone, Default)]
pub struct ItemFilter {
    pub status: Option<String>,
    /// Restrict to these rooms (`None` = all rooms of the workspace).
    pub room_ids: Option<Vec<Uuid>>,
    /// Case-insensitive exact owner names (`None` = any owner).
    pub owner_names: Option<Vec<String>>,
    pub limit: i64,
}

#[derive(Clone)]
pub struct PgSummaryRepository {
    pool: PgPool,
}

impl PgSummaryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// When the room's first summary job was created (the "now" of its prior context).
    pub async fn first_created_at(&self, room: RoomId) -> AppResult<Option<DateTime<Utc>>> {
        sqlx::query_scalar(
            "SELECT MIN(created_at) FROM assistant.meeting_summaries WHERE room_id = $1",
        )
        .bind(room.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)
    }

    /// Stamp the room facts the meeting memory selects on (host, series, title).
    pub async fn stamp_room(
        &self,
        id: Uuid,
        host: Option<Uuid>,
        series: Option<Uuid>,
        title: &str,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.meeting_summaries SET host_user_id = $2, series_id = $3, title = $4 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(host)
        .bind(series)
        .bind(truncate(title, 255))
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    /// Insert the next version of a room's summary as `pending`. An automatic job is unique per
    /// (room, transcript version): a second one returns the existing row.
    pub async fn enqueue(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        transcript_version: i32,
        trigger: Trigger,
        requested_by: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> AppResult<Enqueued> {
        let sql = format!(
            "INSERT INTO assistant.meeting_summaries \
               (id, workspace_id, room_id, version, transcript_version, trigger, requested_by, \
                created_at, updated_at) \
             SELECT $1, $2, $3, COALESCE(MAX(version), 0) + 1, $4, $5, $6, $7, $7 \
             FROM assistant.meeting_summaries WHERE room_id = $3 \
             ON CONFLICT (room_id, transcript_version) WHERE trigger = 'auto' DO NOTHING \
             RETURNING {JOB_COLS}"
        );
        for _ in 0..8 {
            let res = sqlx::query_as::<_, JobRow>(&sql)
                .bind(Uuid::now_v7())
                .bind(ws.as_uuid())
                .bind(room.as_uuid())
                .bind(transcript_version)
                .bind(trigger.as_str())
                .bind(requested_by)
                .bind(now)
                .fetch_optional(&self.pool)
                .await;
            match res {
                Ok(Some(row)) => return Ok(Enqueued::Created(row.try_into()?)),
                Ok(None) => {
                    let existing = sqlx::query_as::<_, JobRow>(&format!(
                        "SELECT {JOB_COLS} FROM assistant.meeting_summaries \
                         WHERE room_id = $1 AND transcript_version = $2 AND trigger = 'auto'"
                    ))
                    .bind(room.as_uuid())
                    .bind(transcript_version)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(map_sqlx_error)?;
                    return Ok(Enqueued::Existing(existing.try_into()?));
                }
                // Lost the race for the version number: take the next one.
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(map_sqlx_error(e)),
            }
        }
        Err(AppError::internal("could not allocate a summary version"))
    }

    pub async fn get(&self, id: Uuid) -> AppResult<Option<SummaryJob>> {
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM assistant.meeting_summaries WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(SummaryJob::try_from)
        .transpose()
    }

    /// Highest version of the room (any status).
    pub async fn latest(&self, room: RoomId) -> AppResult<Option<SummaryJob>> {
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM assistant.meeting_summaries WHERE room_id = $1 \
             ORDER BY version DESC LIMIT 1"
        ))
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(SummaryJob::try_from)
        .transpose()
    }

    pub async fn latest_completed(&self, room: RoomId) -> AppResult<Option<SummaryJob>> {
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM assistant.meeting_summaries \
             WHERE room_id = $1 AND status = 'completed' ORDER BY version DESC LIMIT 1"
        ))
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(SummaryJob::try_from)
        .transpose()
    }

    /// Take a `pending` job (or a `running` one that went quiet) for this process.
    pub async fn claim(
        &self,
        id: Uuid,
        stale: Duration,
        now: DateTime<Utc>,
    ) -> AppResult<Option<SummaryJob>> {
        sqlx::query_as::<_, JobRow>(&format!(
            "UPDATE assistant.meeting_summaries \
             SET status = 'running', attempts = attempts + 1, started_at = $2, updated_at = $2, \
                 error_code = NULL, error = NULL \
             WHERE id = $1 AND (status = 'pending' OR (status = 'running' AND updated_at < $3)) \
             RETURNING {JOB_COLS}"
        ))
        .bind(id)
        .bind(now)
        .bind(now - stale)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(SummaryJob::try_from)
        .transpose()
    }

    /// Mark completed and replace the job's action items, atomically. Items the user already
    /// marked done in the previous completed version stay done when the task is unchanged.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete(
        &self,
        job: &SummaryJob,
        model: &str,
        prompt_version: &str,
        content: &SummaryContent,
        input_segments: i32,
        tokens: (i64, i64),
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let done = sqlx::query(
            "UPDATE assistant.meeting_summaries SET status = 'completed', model = $2, \
                 prompt_version = $3, content = $4, input_segments = $5, tokens_in = $6, \
                 tokens_out = $7, completed_at = $8, updated_at = $8, error_code = NULL, \
                 error = NULL \
             WHERE id = $1 AND status = 'running'",
        )
        .bind(job.id)
        .bind(model)
        .bind(prompt_version)
        .bind(Json(content))
        .bind(input_segments)
        .bind(tokens.0)
        .bind(tokens.1)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        if done.rows_affected() == 0 {
            // Taken over by another worker meanwhile: its result stands.
            return Ok(());
        }
        let previously_done: Vec<String> = sqlx::query_scalar(
            "SELECT lower(btrim(task)) FROM assistant.action_items \
             WHERE workspace_id = $1 AND room_id = $2 AND status = 'done' AND summary_id <> $3",
        )
        .bind(job.workspace_id.as_uuid())
        .bind(job.room_id.as_uuid())
        .bind(job.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "DELETE FROM assistant.action_items WHERE summary_id = $1 AND workspace_id = $2",
        )
        .bind(job.id)
        .bind(job.workspace_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        for it in &content.action_items {
            let status = if previously_done.contains(&it.task.trim().to_lowercase()) {
                "done"
            } else {
                "open"
            };
            insert_item(&mut tx, job, it, status, now).await?;
        }
        tx.commit().await.map_err(map_sqlx_error)
    }

    pub async fn fail(
        &self,
        id: Uuid,
        code: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.meeting_summaries SET status = 'failed', error_code = $2, \
                 error = $3, completed_at = $4, updated_at = $4 \
             WHERE id = $1 AND status = 'running'",
        )
        .bind(id)
        .bind(code)
        .bind(truncate(error, 1000))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    /// Back to `pending` after a transient failure (the sweeper picks it up again).
    pub async fn requeue(&self, id: Uuid, error: &str, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.meeting_summaries SET status = 'pending', error = $2, updated_at = $3 \
             WHERE id = $1 AND status = 'running'",
        )
        .bind(id)
        .bind(truncate(error, 1000))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    /// Heartbeat of a long running job.
    pub async fn touch(&self, id: Uuid, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.meeting_summaries SET updated_at = $2 WHERE id = $1 AND status = 'running'",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    /// Jobs nobody is working on: `pending` for `quiet`, or `running` and silent for `stale`.
    pub async fn open_jobs(
        &self,
        quiet: Duration,
        stale: Duration,
        now: DateTime<Utc>,
        limit: i64,
        workspace: Option<WorkspaceId>,
    ) -> AppResult<Vec<SummaryJob>> {
        let rows = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM assistant.meeting_summaries \
             WHERE ((status = 'pending' AND updated_at < $1) \
                OR (status = 'running' AND updated_at < $2)) \
               AND ($4::uuid IS NULL OR workspace_id = $4) \
             ORDER BY created_at LIMIT $3"
        ))
        .bind(now - quiet)
        .bind(now - stale)
        .bind(limit)
        .bind(workspace.map(|w| w.as_uuid()))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.into_iter().map(SummaryJob::try_from).collect()
    }

    // ------------------------------------------------------------ action items

    /// Items of the room's latest completed summary.
    pub async fn items_for_room(
        &self,
        ws: WorkspaceId,
        room: RoomId,
    ) -> AppResult<Vec<ActionItem>> {
        let rows = sqlx::query_as::<_, ItemRow>(&format!(
            "SELECT {ITEM_COLS} FROM assistant.action_items \
             WHERE workspace_id = $1 AND room_id = $2 AND summary_id = ( \
                 SELECT id FROM assistant.meeting_summaries \
                 WHERE room_id = $2 AND workspace_id = $1 AND status = 'completed' \
                 ORDER BY version DESC LIMIT 1) \
             ORDER BY created_at, id"
        ))
        .bind(ws.as_uuid())
        .bind(room.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(rows.into_iter().map(ActionItem::from).collect())
    }

    pub async fn item(&self, ws: WorkspaceId, id: Uuid) -> AppResult<Option<ActionItem>> {
        Ok(sqlx::query_as::<_, ItemRow>(&format!(
            "SELECT {ITEM_COLS} FROM assistant.action_items WHERE workspace_id = $1 AND id = $2"
        ))
        .bind(ws.as_uuid())
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(ActionItem::from))
    }

    pub async fn update_item(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        patch: &ItemPatch,
        now: DateTime<Utc>,
    ) -> AppResult<Option<ActionItem>> {
        let (set_owner, owner) = match &patch.owner {
            Some(v) => (true, v.clone()),
            None => (false, None),
        };
        let (set_due, due) = match &patch.due {
            Some(v) => (true, v.clone()),
            None => (false, None),
        };
        Ok(sqlx::query_as::<_, ItemRow>(&format!(
            "UPDATE assistant.action_items SET \
                 task = COALESCE($3, task), \
                 owner = CASE WHEN $4 THEN $5 ELSE owner END, \
                 assignee_user_id = CASE WHEN $4 THEN NULL ELSE assignee_user_id END, \
                 due = CASE WHEN $6 THEN $7 ELSE due END, \
                 status = COALESCE($8, status), \
                 updated_at = $9 \
             WHERE workspace_id = $1 AND id = $2 RETURNING {ITEM_COLS}"
        ))
        .bind(ws.as_uuid())
        .bind(id)
        .bind(patch.task.as_deref())
        .bind(set_owner)
        .bind(owner)
        .bind(set_due)
        .bind(due)
        .bind(patch.status.as_deref())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(ActionItem::from))
    }

    /// Items assigned to `user` in the latest completed summaries of `workspaces` (the caller
    /// has checked membership), newest first.
    pub async fn items_for_assignee(
        &self,
        user: Uuid,
        workspaces: &[Uuid],
        status: Option<&str>,
        limit: i64,
    ) -> AppResult<Vec<ActionItem>> {
        let rows = sqlx::query_as::<_, ItemRow>(&format!(
            "SELECT {ITEM_COLS} FROM assistant.action_items a \
             WHERE a.assignee_user_id = $1 AND a.workspace_id = ANY($2) \
               AND ($3::text IS NULL OR a.status = $3) \
               AND a.summary_id = ( \
                   SELECT s.id FROM assistant.meeting_summaries s \
                   WHERE s.room_id = a.room_id AND s.workspace_id = a.workspace_id \
                     AND s.status = 'completed' ORDER BY s.version DESC LIMIT 1) \
             ORDER BY a.created_at DESC, a.id LIMIT $4"
        ))
        .bind(user)
        .bind(workspaces)
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(rows.into_iter().map(ActionItem::from).collect())
    }

    /// Workspaces in which `user` has assigned action items.
    pub async fn assignee_workspaces(&self, user: Uuid) -> AppResult<Vec<Uuid>> {
        sqlx::query_scalar(
            "SELECT DISTINCT workspace_id FROM assistant.action_items WHERE assignee_user_id = $1",
        )
        .bind(user)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)
    }

    /// Items of the workspace's latest completed summaries, newest first.
    pub async fn items_for_workspace(
        &self,
        ws: WorkspaceId,
        f: &ItemFilter,
    ) -> AppResult<Vec<ActionItem>> {
        let owners: Option<Vec<String>> = f
            .owner_names
            .as_ref()
            .map(|v| v.iter().map(|n| n.trim().to_lowercase()).collect());
        let rows = sqlx::query_as::<_, ItemRow>(&format!(
            "SELECT {ITEM_COLS} FROM assistant.action_items a \
             WHERE a.workspace_id = $1 \
               AND ($2::text IS NULL OR a.status = $2) \
               AND ($3::uuid[] IS NULL OR a.room_id = ANY($3)) \
               AND ($4::text[] IS NULL OR lower(btrim(a.owner)) = ANY($4)) \
               AND a.summary_id = ( \
                   SELECT s.id FROM assistant.meeting_summaries s \
                   WHERE s.room_id = a.room_id AND s.workspace_id = a.workspace_id \
                     AND s.status = 'completed' ORDER BY s.version DESC LIMIT 1) \
             ORDER BY a.created_at DESC, a.id LIMIT $5"
        ))
        .bind(ws.as_uuid())
        .bind(f.status.as_deref())
        .bind(f.room_ids.as_deref())
        .bind(owners.as_deref())
        .bind(f.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(rows.into_iter().map(ActionItem::from).collect())
    }
}

async fn insert_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &SummaryJob,
    it: &ActionItemDraft,
    status: &str,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO assistant.action_items \
           (id, workspace_id, room_id, summary_id, task, owner, due, status, source_segment_ids, \
            created_at, updated_at, assignee_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, $11)",
    )
    .bind(Uuid::now_v7())
    .bind(job.workspace_id.as_uuid())
    .bind(job.room_id.as_uuid())
    .bind(job.id)
    .bind(&it.task)
    .bind(it.owner.as_deref())
    .bind(it.due.as_deref())
    .bind(status)
    .bind(&it.source_segment_ids)
    .bind(now)
    .bind(it.assignee_user_id)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
