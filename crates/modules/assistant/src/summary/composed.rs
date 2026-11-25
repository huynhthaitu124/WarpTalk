//! Room/identity/copilot facts through ports (no cross-schema SQL).
//!
//! `apps/api` and `apps/worker` implement the narrow ports below over the owning modules' query
//! facades (`warptalk_room::RoomReader`, `warptalk_identity::UserDirectory`,
//! `warptalk_copilot::CopilotQueries`, `warptalk_workspace::WorkspaceQueries`); this module
//! composes them into the [`RoomMetaSource`] and [`RoomAccess`] the summary service uses.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId};

use super::ports::{
    CopilotNames, RoomAccess, RoomAccessInfo, RoomContext, RoomMeta, RoomMetaSource, WorkspaceScope,
};
use crate::application::ports::WorkspaceAccess;

/// Room type of copilot rooms (their speaker names come from the copilot session).
const COPILOT_ROOM_TYPE: &str = "COPILOT_SESSION";

/// Descriptive facts of a room (owned by the room module).
#[derive(Debug, Clone)]
pub struct RoomFactsData {
    pub workspace_id: WorkspaceId,
    pub host_id: UserId,
    pub title: String,
    pub room_type: String,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub series_id: Option<Uuid>,
    /// Display name and optional account, oldest first.
    pub participants: Vec<(String, Option<UserId>)>,
}

#[async_trait]
pub trait RoomFactsSource: Send + Sync {
    async fn room_facts(&self, room: RoomId) -> AppResult<Option<RoomFactsData>>;
}

/// A user's relation to a room (owned by the room module).
#[derive(Debug, Clone, Copy)]
pub struct RoomRelation {
    pub workspace_id: WorkspaceId,
    pub is_host: bool,
    pub is_participant: bool,
}

#[async_trait]
pub trait RoomRelations: Send + Sync {
    async fn relation(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomRelation>>;
    /// Rooms of the workspace the user hosts or attended, and the names they used there.
    async fn user_scope(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<(Vec<RoomId>, Vec<String>)>;
}

/// Real display names of users (identity module).
#[async_trait]
pub trait HostDirectory: Send + Sync {
    async fn display_name(&self, user: UserId) -> AppResult<Option<String>>;
}

/// Speaker names of a copilot session's two channels (copilot module).
#[async_trait]
pub trait CopilotNameSource: Send + Sync {
    async fn names(&self, room: RoomId) -> AppResult<Option<CopilotNames>>;
}

/// [`RoomMetaSource`] over the ports above.
pub struct ComposedRoomMetaSource {
    pub rooms: Arc<dyn RoomFactsSource>,
    pub hosts: Arc<dyn HostDirectory>,
    pub copilot: Arc<dyn CopilotNameSource>,
}

#[async_trait]
impl RoomMetaSource for ComposedRoomMetaSource {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomMeta>> {
        Ok(self.rooms.room_facts(room).await?.map(|f| RoomMeta {
            workspace_id: f.workspace_id,
            title: f.title,
            source_language: f.source_language,
            target_languages: f.target_languages,
            participants: f.participants.into_iter().map(|(n, _)| n).collect(),
        }))
    }

    async fn context(&self, room: RoomId) -> AppResult<Option<RoomContext>> {
        let Some(f) = self.rooms.room_facts(room).await? else {
            return Ok(None);
        };
        let host_name = self.hosts.display_name(f.host_id).await?;
        let copilot = if f.room_type == COPILOT_ROOM_TYPE {
            Some(
                self.copilot
                    .names(room)
                    .await?
                    .unwrap_or_else(|| CopilotNames {
                        me: "Tôi".into(),
                        others: "Người khác".into(),
                    }),
            )
        } else {
            None
        };
        Ok(Some(RoomContext {
            host_user_id: Some(f.host_id),
            host_name: host_name.filter(|n| !n.trim().is_empty()),
            copilot,
            series_id: f.series_id,
            title: f.title,
            participant_users: f
                .participants
                .into_iter()
                .filter_map(|(n, u)| u.map(|u| (n, u)))
                .collect(),
        }))
    }
}

/// [`RoomAccess`] over the room relation port plus workspace membership.
pub struct ComposedRoomAccess {
    rooms: Arc<dyn RoomRelations>,
    members: Arc<dyn WorkspaceAccess>,
}

impl ComposedRoomAccess {
    pub fn new(rooms: Arc<dyn RoomRelations>, members: Arc<dyn WorkspaceAccess>) -> Self {
        Self { rooms, members }
    }
}

#[async_trait]
impl RoomAccess for ComposedRoomAccess {
    async fn room_access(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomAccessInfo>> {
        let Some(rel) = self.rooms.relation(room, user).await? else {
            return Ok(None);
        };
        Ok(Some(RoomAccessInfo {
            workspace_id: rel.workspace_id,
            is_host: rel.is_host,
            is_participant: rel.is_participant,
            workspace_role: self.members.member_role(rel.workspace_id, user).await?,
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
        let (room_ids, names) = self.rooms.user_scope(workspace, user).await?;
        Ok(Some(WorkspaceScope {
            role,
            room_ids,
            names,
        }))
    }
}
