//! Ports of the glossary use cases.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};

use crate::domain::glossary::{
    GlobalAudit, GlobalStatus, GlobalTerm, GlobalTermFields, Glossary, GlossaryTerm, SnapshotTerm,
};

/// Membership and role of a user in a workspace (adapter over the workspace module).
#[async_trait]
pub trait WorkspaceRoles: Send + Sync {
    /// `None` when the user is not an active member.
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
}

/// No one is a member of any workspace (default until roles are wired).
pub struct DenyAllRoles;

#[async_trait]
impl WorkspaceRoles for DenyAllRoles {
    async fn member_role(&self, _: WorkspaceId, _: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(None)
    }
}

/// Roles fixed per (workspace, user); for tests.
#[derive(Default)]
pub struct InMemoryRoles {
    roles: Mutex<Vec<(WorkspaceId, UserId, WorkspaceRole)>>,
}

impl InMemoryRoles {
    pub fn set(&self, ws: WorkspaceId, user: UserId, role: WorkspaceRole) {
        self.roles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((ws, user, role));
    }
}

#[async_trait]
impl WorkspaceRoles for InMemoryRoles {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(self
            .roles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .find(|(w, u, _)| *w == ws && *u == user)
            .map(|(_, _, r)| *r))
    }
}

#[derive(Debug, Clone)]
pub struct NewGlossary {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub actor: UserId,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct GlossaryPatch {
    pub name: Option<String>,
    /// `Some(None)` clears it.
    pub description: Option<Option<String>>,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TermFields {
    pub source_term: String,
    pub target_term: String,
    pub case_sensitive: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOutcome {
    pub imported: usize,
    pub skipped_existing: usize,
}

/// A global-term change with the audit row that must be written in the same transaction.
#[derive(Debug, Clone)]
pub struct GlobalAuditEntry {
    pub action: &'static str,
    pub actor: UserId,
    pub now: DateTime<Utc>,
}

#[derive(Debug)]
pub enum PublishOutcome {
    Published(Box<GlobalTerm>),
    /// The cap of published terms is reached.
    CapReached,
    NotFound,
}

#[async_trait]
pub trait GlossaryRepo: Send + Sync {
    async fn create_glossary(&self, g: &NewGlossary) -> AppResult<Glossary>;
    async fn glossary(&self, id: Uuid) -> AppResult<Option<Glossary>>;
    async fn glossaries_by_workspace(&self, ws: WorkspaceId) -> AppResult<Vec<Glossary>>;
    async fn update_glossary(
        &self,
        id: Uuid,
        patch: &GlossaryPatch,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Option<Glossary>>;
    /// Soft delete. False when absent.
    async fn delete_glossary(&self, id: Uuid, actor: UserId, now: DateTime<Utc>)
    -> AppResult<bool>;

    async fn terms(&self, glossary: Uuid) -> AppResult<Vec<GlossaryTerm>>;
    async fn term(&self, glossary: Uuid, term: Uuid) -> AppResult<Option<GlossaryTerm>>;
    /// Conflict (`DUPLICATE_TERM`) when the source term exists (case-insensitive).
    async fn add_term(
        &self,
        glossary: Uuid,
        t: &TermFields,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<GlossaryTerm>;
    async fn update_term(
        &self,
        glossary: Uuid,
        term: Uuid,
        t: &TermFields,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Option<GlossaryTerm>>;
    async fn delete_term(
        &self,
        glossary: Uuid,
        term: Uuid,
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<bool>;
    /// Inserts the pairs whose source term does not exist yet, in one transaction.
    async fn import_terms(
        &self,
        glossary: Uuid,
        pairs: &[(String, String)],
        actor: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<ImportOutcome>;
    /// Terms of the workspace's live glossaries, for the pipeline snapshot.
    async fn workspace_snapshot(&self, ws: WorkspaceId) -> AppResult<Vec<SnapshotTerm>>;

    // ---- global glossary
    async fn global_terms(
        &self,
        status: Option<GlobalStatus>,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<GlobalTerm>, i64)>;
    async fn global_term(&self, id: Uuid) -> AppResult<Option<GlobalTerm>>;
    /// Draft term plus `created` audit row. `DUPLICATE_TERM` (400) on the (term, source, target, domain) key.
    async fn create_global(
        &self,
        id: Uuid,
        f: &GlobalTermFields,
        audit: &GlobalAuditEntry,
    ) -> AppResult<GlobalTerm>;
    /// Updates fields, bumps `version`, audits `updated`.
    async fn update_global(
        &self,
        id: Uuid,
        f: &GlobalTermFields,
        audit: &GlobalAuditEntry,
    ) -> AppResult<Option<GlobalTerm>>;
    /// Moves to `published` (needs a definition, checked by the caller) under the cap, audits `published`.
    async fn publish_global(&self, id: Uuid, audit: &GlobalAuditEntry)
    -> AppResult<PublishOutcome>;
    /// Moves to `archived`, audits `archived`.
    async fn archive_global(
        &self,
        id: Uuid,
        audit: &GlobalAuditEntry,
    ) -> AppResult<Option<GlobalTerm>>;
    /// Hard delete, audits `deleted`. False when absent.
    async fn delete_global(&self, id: Uuid, audit: &GlobalAuditEntry) -> AppResult<bool>;
    async fn global_audits(&self, term: Uuid) -> AppResult<Vec<GlobalAudit>>;
    async fn published_global_snapshot(&self) -> AppResult<Vec<SnapshotTerm>>;
}

/// What changed, announced on `warptalk:glossary:changed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlossaryScope {
    Workspace(WorkspaceId),
    Global,
}

/// Writes the pipeline's glossary snapshots.
#[async_trait]
pub trait GlossaryFeed: Send + Sync {
    /// Replaces the snapshot of `scope` and announces the change. Returns the new version.
    async fn publish(&self, scope: &GlossaryScope, terms: &[SnapshotTerm]) -> AppResult<i64>;
}

/// Feed that drops everything.
pub struct NoopGlossaryFeed;

#[async_trait]
impl GlossaryFeed for NoopGlossaryFeed {
    async fn publish(&self, _: &GlossaryScope, _: &[SnapshotTerm]) -> AppResult<i64> {
        Ok(0)
    }
}
