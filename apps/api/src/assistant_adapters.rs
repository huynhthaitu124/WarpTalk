//! Adapters of the assistant's summary ports over the owning modules' query facades
//! (room, identity, copilot, workspace). No cross-schema SQL lives here or in the assistant.

use std::sync::Arc;

use async_trait::async_trait;
use warptalk_assistant::summary::{
    ComposedRoomMetaSource, CopilotNameSource, CopilotNames, ExternalLlmGate, HostDirectory,
    RoomFactsData, RoomFactsSource, RoomRelation, RoomRelations,
};
use warptalk_copilot::CopilotQueries;
use warptalk_identity::IdentityQueries;
use warptalk_kernel::{AppResult, RoomId, UserId, WorkspaceId};
use warptalk_room::RoomReader;
use warptalk_workspace::WorkspaceQueries;

pub struct RoomFactsAdapter(pub RoomReader);

#[async_trait]
impl RoomFactsSource for RoomFactsAdapter {
    async fn room_facts(&self, room: RoomId) -> AppResult<Option<RoomFactsData>> {
        Ok(self.0.room_facts(room).await?.map(|f| RoomFactsData {
            workspace_id: f.workspace_id,
            host_id: f.host_id,
            title: f.title,
            room_type: f.room_type,
            source_language: f.source_language,
            target_languages: f.target_languages,
            series_id: f.series_id,
            participants: f
                .participants
                .into_iter()
                .map(|p| (p.display_name, p.user_id))
                .collect(),
        }))
    }
}

#[async_trait]
impl RoomRelations for RoomFactsAdapter {
    async fn relation(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomRelation>> {
        Ok(self
            .0
            .user_relation(room, user)
            .await?
            .map(|r| RoomRelation {
                workspace_id: r.workspace_id,
                is_host: r.is_host,
                is_participant: r.is_participant,
            }))
    }

    async fn user_scope(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<(Vec<RoomId>, Vec<String>)> {
        self.0.user_scope(ws, user).await
    }
}

pub struct IdentityHostDirectory(pub IdentityQueries);

#[async_trait]
impl HostDirectory for IdentityHostDirectory {
    async fn display_name(&self, user: UserId) -> AppResult<Option<String>> {
        Ok(self.0.user_summary(user).await?.map(|u| u.full_name))
    }
}

pub struct CopilotNamesAdapter(pub CopilotQueries);

#[async_trait]
impl CopilotNameSource for CopilotNamesAdapter {
    async fn names(&self, room: RoomId) -> AppResult<Option<CopilotNames>> {
        Ok(self.0.session_names(room).await?.map(|n| CopilotNames {
            me: n.me,
            others: n.others,
        }))
    }
}

/// `externalLlmAllowed` from the workspace module's AI policy. Fail-closed.
pub struct WorkspaceExternalLlmGate(pub WorkspaceQueries);

#[async_trait]
impl ExternalLlmGate for WorkspaceExternalLlmGate {
    async fn external_llm_allowed(&self, workspace: WorkspaceId) -> bool {
        match self.0.ai_policy(workspace).await {
            Ok(p) => p.external_llm_allowed,
            Err(e) => {
                tracing::warn!(error = %e, "external LLM policy lookup failed; denying");
                false
            }
        }
    }
}

/// The summary's room metadata source over the module facades.
pub fn room_meta_source(
    room: RoomReader,
    identity: IdentityQueries,
    copilot: CopilotQueries,
) -> ComposedRoomMetaSource {
    ComposedRoomMetaSource {
        rooms: Arc::new(RoomFactsAdapter(room)),
        hosts: Arc::new(IdentityHostDirectory(identity)),
        copilot: Arc::new(CopilotNamesAdapter(copilot)),
    }
}
