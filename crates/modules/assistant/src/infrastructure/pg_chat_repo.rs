//! `assistant.assistant_conversations|messages|tool_calls` over runtime sqlx queries.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};

use crate::application::chat_ports::ConversationRepository;
use crate::domain::chat::{ChatMessage, Conversation, MessageRole, MessageStatus, ToolCallRecord};

const CONV_COLS: &str =
    "id, workspace_id, user_id, title, created_at, last_message_at, is_archived";
const MSG_COLS: &str = "id, conversation_id, workspace_id, user_id, role, content, status, \
     created_at, completed_at, tool_results_json";

#[derive(sqlx::FromRow)]
struct ConvRow {
    id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    title: String,
    created_at: DateTime<Utc>,
    last_message_at: Option<DateTime<Utc>>,
    is_archived: bool,
}

impl From<ConvRow> for Conversation {
    fn from(r: ConvRow) -> Self {
        Self {
            id: r.id,
            workspace_id: WorkspaceId::from_uuid(r.workspace_id),
            user_id: UserId::from_uuid(r.user_id),
            title: r.title,
            created_at: r.created_at,
            last_message_at: r.last_message_at,
            is_archived: r.is_archived,
        }
    }
}

#[derive(sqlx::FromRow)]
struct MsgRow {
    id: Uuid,
    conversation_id: Uuid,
    workspace_id: Uuid,
    user_id: Option<Uuid>,
    role: String,
    content: String,
    status: String,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    tool_results_json: Option<String>,
}

impl TryFrom<MsgRow> for ChatMessage {
    type Error = AppError;
    fn try_from(r: MsgRow) -> AppResult<Self> {
        Ok(Self {
            id: r.id,
            conversation_id: r.conversation_id,
            workspace_id: WorkspaceId::from_uuid(r.workspace_id),
            user_id: r.user_id.map(UserId::from_uuid),
            role: MessageRole::parse(&r.role)
                .ok_or_else(|| AppError::internal(format!("bad message role {}", r.role)))?,
            content: r.content,
            status: MessageStatus::parse(&r.status)
                .ok_or_else(|| AppError::internal(format!("bad message status {}", r.status)))?,
            created_at: r.created_at,
            completed_at: r.completed_at,
            sources_json: r.tool_results_json,
        })
    }
}

pub struct PgConversationRepository {
    pool: PgPool,
}

impl PgConversationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConversationRepository for PgConversationRepository {
    async fn create(&self, c: &Conversation) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO assistant.assistant_conversations \
             (id, workspace_id, user_id, title, created_at, last_message_at, is_archived) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(c.id)
        .bind(c.workspace_id.as_uuid())
        .bind(c.user_id.as_uuid())
        .bind(&c.title)
        .bind(c.created_at)
        .bind(c.last_message_at)
        .bind(c.is_archived)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn list(&self, ws: WorkspaceId, user: UserId) -> AppResult<Vec<Conversation>> {
        let rows: Vec<ConvRow> = sqlx::query_as(&format!(
            "SELECT {CONV_COLS} FROM assistant.assistant_conversations \
             WHERE workspace_id = $1 AND user_id = $2 AND NOT is_archived \
             ORDER BY COALESCE(last_message_at, created_at) DESC LIMIT 200"
        ))
        .bind(ws.as_uuid())
        .bind(user.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn find_owned(&self, user: UserId, id: Uuid) -> AppResult<Option<Conversation>> {
        let row: Option<ConvRow> = sqlx::query_as(&format!(
            "SELECT {CONV_COLS} FROM assistant.assistant_conversations \
             WHERE id = $1 AND user_id = $2"
        ))
        .bind(id)
        .bind(user.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(row.map(Into::into))
    }

    async fn archive(&self, ws: WorkspaceId, user: UserId, id: Uuid) -> AppResult<bool> {
        let r = sqlx::query(
            "UPDATE assistant.assistant_conversations SET is_archived = true \
             WHERE workspace_id = $1 AND user_id = $2 AND id = $3",
        )
        .bind(ws.as_uuid())
        .bind(user.as_uuid())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_title(&self, ws: WorkspaceId, id: Uuid, title: &str) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.assistant_conversations SET title = $3 \
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(ws.as_uuid())
        .bind(id)
        .bind(title)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn touch(&self, ws: WorkspaceId, id: Uuid, at: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.assistant_conversations SET last_message_at = $3 \
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(ws.as_uuid())
        .bind(id)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn insert_message(&self, m: &ChatMessage) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO assistant.assistant_messages \
             (id, conversation_id, workspace_id, user_id, role, content, status, created_at, \
              completed_at, tool_results_json) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(m.id)
        .bind(m.conversation_id)
        .bind(m.workspace_id.as_uuid())
        .bind(m.user_id.map(|u| u.as_uuid()))
        .bind(m.role.as_str())
        .bind(&m.content)
        .bind(m.status.as_str())
        .bind(m.created_at)
        .bind(m.completed_at)
        .bind(m.sources_json.as_deref())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn list_messages(
        &self,
        ws: WorkspaceId,
        conversation: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        let rows: Vec<MsgRow> = sqlx::query_as(&format!(
            "SELECT {MSG_COLS} FROM assistant.assistant_messages \
             WHERE workspace_id = $1 AND conversation_id = $2 ORDER BY created_at, id"
        ))
        .bind(ws.as_uuid())
        .bind(conversation)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn get_message(&self, ws: WorkspaceId, id: Uuid) -> AppResult<Option<ChatMessage>> {
        let row: Option<MsgRow> = sqlx::query_as(&format!(
            "SELECT {MSG_COLS} FROM assistant.assistant_messages \
             WHERE workspace_id = $1 AND id = $2"
        ))
        .bind(ws.as_uuid())
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.map(TryInto::try_into).transpose()
    }

    async fn finish_message(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        status: MessageStatus,
        content: &str,
        sources_json: Option<&str>,
        at: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.assistant_messages \
             SET status = $3, content = $4, tool_results_json = $5, completed_at = $6 \
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(ws.as_uuid())
        .bind(id)
        .bind(status.as_str())
        .bind(content)
        .bind(sources_json)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn insert_tool_call(&self, t: &ToolCallRecord) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO assistant.assistant_tool_calls \
             (id, message_id, tool_name, arguments_json, status, created_at) \
             VALUES ($1, $2, $3, $4, 'pending', $5)",
        )
        .bind(t.id)
        .bind(t.message_id)
        .bind(&t.tool_name)
        .bind(&t.arguments_json)
        .bind(t.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn finish_tool_call(
        &self,
        id: Uuid,
        ok: bool,
        result_json: &str,
        at: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE assistant.assistant_tool_calls \
             SET status = $2, result_json = $3, completed_at = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(if ok { "completed" } else { "failed" })
        .bind(result_json)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }
}
