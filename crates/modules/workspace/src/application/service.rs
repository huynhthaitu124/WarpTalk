//! Workspace use cases.

use std::sync::Arc;

use warptalk_kernel::{AppResult, Clock, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole};

use super::ports::{
    MemberRecord, NewWorkspace, SettingsObserver, UserDirectory, UserSummary, WorkspaceRecord,
    WorkspaceRepo,
};
use crate::domain::{self, WorkspaceSettings};

pub struct WorkspaceService {
    repo: Arc<dyn WorkspaceRepo>,
    users: Arc<dyn UserDirectory>,
    clock: Arc<dyn Clock>,
    observer: std::sync::OnceLock<Arc<dyn SettingsObserver>>,
}

pub struct MemberView {
    pub record: MemberRecord,
    pub user: Option<UserSummary>,
}

impl WorkspaceService {
    pub fn new(
        repo: Arc<dyn WorkspaceRepo>,
        users: Arc<dyn UserDirectory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            users,
            clock,
            observer: std::sync::OnceLock::new(),
        }
    }

    /// Install the settings observer (once; later calls are ignored).
    pub fn set_observer(&self, observer: Arc<dyn SettingsObserver>) {
        let _ = self.observer.set(observer);
    }

    pub async fn create(
        &self,
        owner: UserId,
        name: &str,
        description: Option<&str>,
    ) -> AppResult<WorkspaceRecord> {
        let name = domain::validate_name(name)?;
        let description = domain::validate_description(description)?;
        let id = WorkspaceId::new();
        let suffix: String = id.to_string().chars().rev().take(8).collect();
        let slug = format!("{}-{suffix}", domain::slugify(&name));
        self.repo
            .create(NewWorkspace {
                id,
                name,
                slug,
                description,
                owner,
                now: self.clock.now(),
            })
            .await
    }

    pub async fn list(
        &self,
        user: UserId,
        search: Option<&str>,
        page: &PageRequest,
    ) -> AppResult<Page<WorkspaceRecord>> {
        let search = search.map(str::trim).filter(|s| !s.is_empty());
        self.repo.list_for_user(user, search, page).await
    }

    pub async fn get(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRecord> {
        self.repo.get_for_member(ws, user).await
    }

    pub async fn delete(&self, ws: WorkspaceId, actor: UserId) -> AppResult<()> {
        self.repo.delete(ws, actor, self.clock.now()).await
    }

    pub async fn get_settings(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<WorkspaceSettings> {
        self.repo.get_settings(ws, user).await
    }

    pub async fn update_settings(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        settings: WorkspaceSettings,
    ) -> AppResult<WorkspaceSettings> {
        settings.validate()?;
        self.repo
            .update_settings(ws, actor, &settings, self.clock.now())
            .await?;
        if let Some(observer) = self.observer.get() {
            observer.settings_changed(ws).await;
        }
        self.repo.get_settings(ws, actor).await
    }

    pub async fn list_members(
        &self,
        ws: WorkspaceId,
        user: UserId,
        page: &PageRequest,
    ) -> AppResult<Page<MemberView>> {
        let p = self.repo.list_members(ws, user, page).await?;
        let ids: Vec<UserId> = p.items.iter().map(|m| m.user_id).collect();
        let mut dir = self.users.summaries(&ids).await?;
        let items = p
            .items
            .into_iter()
            .map(|record| MemberView {
                user: dir.remove(&record.user_id),
                record,
            })
            .collect();
        Ok(Page {
            items,
            page: p.page,
            page_size: p.page_size,
            total: p.total,
        })
    }

    pub async fn change_role(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
        role: WorkspaceRole,
    ) -> AppResult<()> {
        self.repo.change_role(ws, actor, target, role).await
    }

    pub async fn remove_member(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
    ) -> AppResult<()> {
        self.repo
            .remove_member(ws, actor, target, self.clock.now())
            .await
    }

    pub async fn leave(&self, ws: WorkspaceId, actor: UserId) -> AppResult<()> {
        self.repo.leave(ws, actor, self.clock.now()).await
    }

    pub async fn transfer_ownership(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        new_owner: UserId,
    ) -> AppResult<()> {
        self.repo
            .transfer_ownership(ws, actor, new_owner, self.clock.now())
            .await
    }
}
