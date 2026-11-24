//! sqlx adapter for `copilot.sessions`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, RoomId, UserId, WorkspaceId};

use crate::application::ports::{SessionRecord, SessionRepo};
use crate::domain::Platform;

pub struct PgSessionRepo {
    pool: PgPool,
}

impl PgSessionRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLS: &str = "room_id, workspace_id, host_id, host_participant_id, others_speaker_id, \
    title, platform, meeting_url, language_me, language_others, name_me, name_others, created_at";

#[derive(sqlx::FromRow)]
struct Row {
    room_id: Uuid,
    workspace_id: Uuid,
    host_id: Uuid,
    host_participant_id: Uuid,
    others_speaker_id: Uuid,
    title: String,
    platform: String,
    meeting_url: Option<String>,
    language_me: String,
    language_others: String,
    name_me: String,
    name_others: String,
    created_at: DateTime<Utc>,
}

impl Row {
    fn into_record(self) -> AppResult<SessionRecord> {
        let platform = Platform::parse(&self.platform)
            .ok_or_else(|| AppError::internal(format!("unknown platform {}", self.platform)))?;
        Ok(SessionRecord {
            room_id: self.room_id.into(),
            workspace_id: self.workspace_id.into(),
            host_id: self.host_id.into(),
            host_participant_id: self.host_participant_id,
            others_speaker_id: self.others_speaker_id,
            title: self.title,
            platform,
            meeting_url: self.meeting_url,
            language_me: self.language_me,
            language_others: self.language_others,
            name_me: self.name_me,
            name_others: self.name_others,
            created_at: self.created_at,
        })
    }
}

#[async_trait]
impl SessionRepo for PgSessionRepo {
    async fn insert(&self, s: &SessionRecord) -> AppResult<()> {
        sqlx::query(&format!(
            "INSERT INTO copilot.sessions ({COLS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"
        ))
        .bind(s.room_id.as_uuid())
        .bind(s.workspace_id.as_uuid())
        .bind(s.host_id.as_uuid())
        .bind(s.host_participant_id)
        .bind(s.others_speaker_id)
        .bind(&s.title)
        .bind(s.platform.as_str())
        .bind(&s.meeting_url)
        .bind(&s.language_me)
        .bind(&s.language_others)
        .bind(&s.name_me)
        .bind(&s.name_others)
        .bind(s.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn get(&self, room: RoomId) -> AppResult<Option<SessionRecord>> {
        sqlx::query_as::<_, Row>(&format!(
            "SELECT {COLS} FROM copilot.sessions WHERE room_id = $1"
        ))
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(Row::into_record)
        .transpose()
    }

    async fn list(
        &self,
        ws: WorkspaceId,
        host: Option<UserId>,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<SessionRecord>, i64)> {
        let host = host.map(|h| h.as_uuid());
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM copilot.sessions \
             WHERE workspace_id = $1 AND ($2::uuid IS NULL OR host_id = $2)",
        )
        .bind(ws.as_uuid())
        .bind(host)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let rows = sqlx::query_as::<_, Row>(&format!(
            "SELECT {COLS} FROM copilot.sessions \
             WHERE workspace_id = $1 AND ($2::uuid IS NULL OR host_id = $2) \
             ORDER BY created_at DESC, room_id DESC LIMIT $3 OFFSET $4"
        ))
        .bind(ws.as_uuid())
        .bind(host)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let items = rows
            .into_iter()
            .map(Row::into_record)
            .collect::<AppResult<Vec<_>>>()?;
        Ok((items, total))
    }
}
