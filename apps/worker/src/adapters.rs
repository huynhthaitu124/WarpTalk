//! Simple worker-side adapters for module ports.
//!
//! TEMPORARY cross-schema reads: these query tables owned by other modules
//! (`translation_room.translation_rooms`, `workspace.workspaces`) with runtime SQL.
//! Replace them with a Redis-cached policy / room lookup fed by those modules.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, WorkspaceId};
use warptalk_transcript::application::ports::{RoomDetails, RoomDirectory, WorkspaceAiPolicy};

/// Looks up a room's workspace in `translation_room.translation_rooms`.
pub struct PgRoomDirectory {
    pool: PgPool,
}

impl PgRoomDirectory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RoomDirectory for PgRoomDirectory {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomDetails>> {
        let ws: Option<Uuid> = sqlx::query_scalar(
            "SELECT workspace_id FROM translation_room.translation_rooms WHERE id = $1",
        )
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(ws.map(|w| RoomDetails {
            workspace_id: WorkspaceId::from_uuid(w),
        }))
    }
}

/// Reads `workspace.workspaces.settings` keys `aiRetrievalAllowed` / `externalLlmAllowed`
/// (jsonb booleans; both default to false when absent or the workspace is gone).
pub struct PgWorkspaceAiPolicy {
    pool: PgPool,
}

impl PgWorkspaceAiPolicy {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn flag(&self, ws: WorkspaceId, key: &str) -> AppResult<bool> {
        let v: Option<bool> = sqlx::query_scalar(
            "SELECT COALESCE(settings -> $2 = 'true'::jsonb, false) \
             FROM workspace.workspaces WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(ws.as_uuid())
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(v.unwrap_or(false))
    }
}

#[async_trait]
impl WorkspaceAiPolicy for PgWorkspaceAiPolicy {
    async fn ai_retrieval_allowed(&self, ws: WorkspaceId) -> AppResult<bool> {
        self.flag(ws, "aiRetrievalAllowed").await
    }

    async fn external_llm_allowed(&self, ws: WorkspaceId) -> AppResult<bool> {
        self.flag(ws, "externalLlmAllowed").await
    }
}
