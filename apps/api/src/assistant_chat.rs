//! Adapters for WarpBot chat: the workspace's assistant model (AI plane), the workspace consent
//! for external models, and permission-scoped knowledge search.

use std::sync::Arc;

use async_trait::async_trait;
use warptalk_ai_plane::ExternalLlmPolicy;
use warptalk_ai_plane::ai::AiProviders;
use warptalk_assistant::application::chat_ports::{
    AssistantLlmResolver, KnowledgeHit, WorkspaceKnowledge, WorkspaceLlmConsent,
};
use warptalk_assistant::domain::byo::LlmPurpose;
use warptalk_assistant::suggestion::{ResolveError, ResolvedLlm};
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::domain::CitationKind;
use warptalk_knowledge::{AccessContext, DocumentAccess, KnowledgeService, SearchFilters};

/// `LlmPurpose::Assistant`: the workspace's own model when configured, else the platform one.
pub struct AiAssistantLlm(pub Arc<AiProviders>);

#[async_trait]
impl AssistantLlmResolver for AiAssistantLlm {
    async fn resolve(&self, ws: WorkspaceId) -> Result<ResolvedLlm, ResolveError> {
        self.0.llm_for_purpose(ws, LlmPurpose::Assistant).await
    }
}

/// `externalLlmAllowed` of the workspace (fails closed).
pub struct PolicyConsent(pub Arc<dyn ExternalLlmPolicy>);

#[async_trait]
impl WorkspaceLlmConsent for PolicyConsent {
    async fn external_llm_allowed(&self, ws: WorkspaceId) -> AppResult<bool> {
        Ok(self.0.external_llm_allowed(ws).await)
    }
}

/// Searches as the asking user: owners/admins are unscoped in their workspace, members are
/// limited to the documents and meetings they may read (same rule as `GET /knowledge/search`).
pub struct KnowledgeSearch {
    pub service: Arc<KnowledgeService>,
    pub access: Arc<dyn DocumentAccess>,
}

#[async_trait]
impl WorkspaceKnowledge for KnowledgeSearch {
    async fn search(
        &self,
        ws: WorkspaceId,
        user: UserId,
        query: &str,
        limit: usize,
    ) -> AppResult<Vec<KnowledgeHit>> {
        let scope = self.access.access_scope(ws, user).await?;
        let unscoped = scope.role.rank() >= WorkspaceRole::Admin.rank();
        let ctx = AccessContext {
            workspace: ws,
            user,
            role: scope.role,
            allowed_document_ids: (!unscoped).then_some(scope.allowed_document_ids),
            allowed_meeting_ids: (!unscoped).then_some(scope.allowed_meeting_ids),
        };
        let filters = SearchFilters {
            limit: Some(limit),
            ..SearchFilters::default()
        };
        let res = self.service.search(&ctx, query, &filters).await?;
        Ok(res
            .chunks
            .into_iter()
            .map(|h| KnowledgeHit {
                title: if h.citation.label.trim().is_empty() {
                    h.citation.title.clone()
                } else {
                    h.citation.label.clone()
                },
                text: h.payload.text.clone(),
                kind: match h.citation.kind {
                    CitationKind::Document => "document",
                    CitationKind::Transcript => "transcript",
                    CitationKind::Meeting => "meeting",
                    CitationKind::Glossary => "glossary",
                    CitationKind::Knowledge => "knowledge",
                }
                .to_owned(),
                ref_id: h.citation.ref_id.clone(),
                score: h.score,
            })
            .collect())
    }
}
