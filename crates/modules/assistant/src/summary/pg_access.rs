//! `RoomAccess` over `translation_room.*` plus the workspace membership port.
//!
//! TEMPORARY cross-schema read (like the worker adapters): replace with a port implemented by
//! the room module once it exposes membership queries.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId};

use super::ports::{RoomAccess, RoomAccessInfo, WorkspaceScope};
use crate::application::ports::WorkspaceAccess;

pub struct PgRoomAccess {
    pool: PgPool,
    members: Arc<dyn WorkspaceAccess>,
}

impl PgRoomAccess {
    pub fn new(pool: PgPool, members: Arc<dyn WorkspaceAccess>) -> Self {
        Self { pool, members }
    }
}

#[async_trait]
impl RoomAccess for PgRoomAccess {
    async fn room_access(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomAccessInfo>> {
        let row: Option<(Uuid, bool, bool)> = sqlx::query_as(
            "SELECT r.workspace_id, \
                    (r.host_id = $2 OR r.active_host_id = $2), \
                    EXISTS (SELECT 1 FROM translation_room.translation_room_participants p \
                            WHERE p.translation_room_id = r.id AND p.user_id = $2) \
             FROM translation_room.translation_rooms r \
             WHERE r.id = $1 AND r.deleted_at IS NULL",
        )
        .bind(room.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some((ws, is_host, is_participant)) = row else {
            return Ok(None);
        };
        let ws = WorkspaceId::from_uuid(ws);
        Ok(Some(RoomAccessInfo {
            workspace_id: ws,
            is_host,
            is_participant,
            workspace_role: self.members.member_role(ws, user).await?,
        }))
    }

    async fn workspace_scope(
        &self,
        workspace: WorkspaceId,
        user: UserId,
    ) -> AppResult<Option<WorkspaceScope>> {
        let Some(role) = self.members.member_role(workspace, user).await? else {
            return Ok(None);
        };
        let rooms: Vec<Uuid> = sqlx::query_scalar(
            "SELECT r.id FROM translation_room.translation_rooms r \
             WHERE r.workspace_id = $1 AND r.deleted_at IS NULL AND ( \
                   r.host_id = $2 OR r.active_host_id = $2 OR EXISTS ( \
                       SELECT 1 FROM translation_room.translation_room_participants p \
                       WHERE p.translation_room_id = r.id AND p.user_id = $2))",
        )
        .bind(workspace.as_uuid())
        .bind(user.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT p.display_name FROM translation_room.translation_room_participants p \
             JOIN translation_room.translation_rooms r ON r.id = p.translation_room_id \
             WHERE r.workspace_id = $1 AND p.user_id = $2",
        )
        .bind(workspace.as_uuid())
        .bind(user.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(Some(WorkspaceScope {
            role,
            room_ids: rooms.into_iter().map(RoomId::from_uuid).collect(),
            names,
        }))
    }
}
