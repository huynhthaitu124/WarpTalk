//! Ports: the user directory (implemented by the api wiring over identity) and the
//! workspace repository (implemented in `infrastructure`).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use warptalk_kernel::{AppResult, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole};

use crate::domain::entitlements::EntitlementSnapshot;
use crate::domain::{AiPolicy, WorkspaceSettings};

#[derive(Debug, Clone)]
pub struct UserSummary {
    pub email: String,
    pub full_name: String,
    pub avatar_url: Option<String>,
}

#[async_trait]
pub trait UserDirectory: Send + Sync {
    async fn summaries(&self, ids: &[UserId]) -> AppResult<HashMap<UserId, UserSummary>>;
}

/// Directory that knows nobody (tests).
pub struct NoopUserDirectory;

#[async_trait]
impl UserDirectory for NoopUserDirectory {
    async fn summaries(&self, _ids: &[UserId]) -> AppResult<HashMap<UserId, UserSummary>> {
        Ok(HashMap::new())
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub owner_id: UserId,
    /// Role of the requesting user.
    pub my_role: WorkspaceRole,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub user_id: UserId,
    pub role: WorkspaceRole,
    pub joined_at: DateTime<Utc>,
    /// `internal` or `external` (05a `workspace_members.membership_type`).
    pub membership_type: String,
}

pub struct NewWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub owner: UserId,
    pub now: DateTime<Utc>,
}

/// Persistence port. Mutating methods run in one transaction, lock the workspace row,
/// re-check the actor's role there and apply the policies from `domain`.
/// Non-members (and inactive workspaces) always yield `NotFound`.
#[async_trait]
pub trait WorkspaceRepo: Send + Sync {
    async fn create(&self, new: NewWorkspace) -> AppResult<WorkspaceRecord>;
    async fn list_for_user(
        &self,
        user: UserId,
        search: Option<&str>,
        page: &PageRequest,
    ) -> AppResult<Page<WorkspaceRecord>>;
    async fn get_for_member(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRecord>;
    async fn delete(&self, ws: WorkspaceId, actor: UserId, now: DateTime<Utc>) -> AppResult<()>;
    async fn get_settings(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceSettings>;
    async fn update_settings(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        settings: &WorkspaceSettings,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn list_members(
        &self,
        ws: WorkspaceId,
        user: UserId,
        page: &PageRequest,
    ) -> AppResult<Page<MemberRecord>>;
    async fn change_role(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
        role: WorkspaceRole,
    ) -> AppResult<()>;
    async fn remove_member(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn leave(&self, ws: WorkspaceId, actor: UserId, now: DateTime<Utc>) -> AppResult<()>;
    async fn transfer_ownership(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        new_owner: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
    async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool>;
    /// Effective AI policy flags; defaults when the workspace is missing or deleted.
    async fn ai_policy(&self, ws: WorkspaceId) -> AppResult<AiPolicy>;
    /// Billing's latest resolved entitlements; `None` until the first snapshot is projected.
    async fn entitlement_snapshot(&self, ws: WorkspaceId)
    -> AppResult<Option<EntitlementSnapshot>>;
    /// Projects a snapshot published by billing (event `event_id`). Out-of-order deliveries are
    /// ignored: a snapshot replaces the stored one only when its `resolved_at` is not older, and a
    /// redelivered event is a no-op. True when the row changed.
    async fn apply_entitlement_snapshot(
        &self,
        ws: WorkspaceId,
        snapshot: &EntitlementSnapshot,
        event_id: uuid::Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<bool>;
}

/// Told after a workspace's settings were written (api wires cache/Redis refreshers).
#[async_trait]
pub trait SettingsObserver: Send + Sync {
    async fn settings_changed(&self, ws: WorkspaceId);
}
