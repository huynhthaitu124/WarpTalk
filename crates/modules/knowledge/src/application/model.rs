//! Query-side types: caller context, filters, results, metrics.

use serde::Serialize;
use uuid::Uuid;
use warptalk_kernel::{UserId, WorkspaceId, WorkspaceRole};

use crate::domain::{ChunkPayload, Citation, SourceType, normalize_id};

/// Who is asking and what they may see.
///
/// `None` allowlists mean "unscoped" and are honoured only for Owner/Admin; for members a
/// missing allowlist is treated as empty (fail closed).
#[derive(Debug, Clone)]
pub struct AccessContext {
    pub workspace: WorkspaceId,
    pub user: UserId,
    pub role: WorkspaceRole,
    pub allowed_document_ids: Option<Vec<Uuid>>,
    pub allowed_meeting_ids: Option<Vec<Uuid>>,
}

/// Allowlists after the role rule was applied (normalised string ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveScope {
    pub documents: Option<Vec<String>>,
    pub meetings: Option<Vec<String>>,
}

impl AccessContext {
    /// Context for an Owner/Admin: unscoped.
    pub fn privileged(workspace: WorkspaceId, user: UserId, role: WorkspaceRole) -> Self {
        Self {
            workspace,
            user,
            role,
            allowed_document_ids: None,
            allowed_meeting_ids: None,
        }
    }

    /// Context for a member limited to the given allowlists.
    pub fn member(
        workspace: WorkspaceId,
        user: UserId,
        documents: Vec<Uuid>,
        meetings: Vec<Uuid>,
    ) -> Self {
        Self {
            workspace,
            user,
            role: WorkspaceRole::Member,
            allowed_document_ids: Some(documents),
            allowed_meeting_ids: Some(meetings),
        }
    }

    pub fn is_privileged(&self) -> bool {
        self.role.rank() >= WorkspaceRole::Admin.rank()
    }

    pub fn effective_scope(&self) -> EffectiveScope {
        let norm = |ids: &Option<Vec<Uuid>>| {
            ids.as_ref()
                .map(|v| v.iter().map(Uuid::to_string).collect::<Vec<_>>())
        };
        if self.is_privileged() {
            EffectiveScope {
                documents: norm(&self.allowed_document_ids),
                meetings: norm(&self.allowed_meeting_ids),
            }
        } else {
            EffectiveScope {
                documents: Some(norm(&self.allowed_document_ids).unwrap_or_default()),
                meetings: Some(norm(&self.allowed_meeting_ids).unwrap_or_default()),
            }
        }
    }
}

impl EffectiveScope {
    /// Defense in depth: would this payload pass the scope filter?
    pub fn admits(&self, p: &ChunkPayload) -> bool {
        match p.source_type {
            SourceType::Document => self
                .documents
                .as_ref()
                .is_none_or(|ids| ids.contains(&normalize_id(&p.source_id))),
            SourceType::Transcript | SourceType::MeetingSummary => {
                self.meetings.as_ref().is_none_or(|ids| {
                    p.meeting_id
                        .as_deref()
                        .is_some_and(|m| ids.contains(&normalize_id(m)))
                })
            }
            SourceType::GlossaryTerm | SourceType::GlobalGlossaryTerm => true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchFilters {
    pub source_types: Vec<SourceType>,
    /// Document page (1-based).
    pub page: Option<u32>,
    pub source_id: Option<String>,
    /// Override `top_k` (clamped to 1..=50).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub chunk_id: Uuid,
    /// Final (re-ranked) score.
    pub score: f32,
    /// Raw vector similarity.
    pub dense_score: f32,
    pub payload: ChunkPayload,
    pub citation: Citation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchMetrics {
    pub latency_ms: u64,
    pub embed_ms: u64,
    /// Hits returned by the vector store.
    pub candidates: usize,
    /// Candidates left after the defense-in-depth check and `min_score`.
    pub kept_after_min_score: usize,
    pub returned: usize,
    /// Top final score of the returned list.
    pub top_score: Option<f32>,
    pub top_dense_score: Option<f32>,
    /// Re-ranker actually used (`off`, `lexical`, `llm`, or `lexical_fallback`).
    pub reranker: String,
    /// Top final score minus top dense score over the kept candidates (0 when not re-ranked).
    pub reranker_score_delta: f32,
    /// Kept candidates whose rank differs between dense order and re-ranked order.
    pub reranker_rank_changes: usize,
    pub zero_result: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunks: Vec<SearchHit>,
    pub metrics: SearchMetrics,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn members_fail_closed_and_admins_stay_unscoped() {
        let (w, u) = (WorkspaceId::new(), UserId::new());
        let mut m = AccessContext::member(w, u, vec![], vec![]);
        m.allowed_document_ids = None;
        assert_eq!(m.effective_scope().documents, Some(vec![]));
        let a = AccessContext::privileged(w, u, WorkspaceRole::Admin);
        assert_eq!(a.effective_scope().documents, None);
        let d = Uuid::now_v7();
        let m = AccessContext::member(w, u, vec![d], vec![]);
        assert_eq!(m.effective_scope().documents, Some(vec![d.to_string()]));
    }
}
