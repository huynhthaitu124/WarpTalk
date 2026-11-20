use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use warptalk_kernel::{Page, UserId, WorkspaceId};

use crate::application::ports::WorkspaceRecord;
use crate::application::service::MemberView;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub description: Option<String>,
}

/// Optional `search` of the workspace list. Paging is extracted separately as `PageRequest`
/// (`#[serde(flatten)]` would buffer `pageSize` as a string and reject it).
#[derive(Deserialize)]
pub struct WorkspaceSearchQuery {
    pub search: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRoleRequest {
    pub role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    pub new_owner_id: UserId,
}

/// Maps the items of a kernel page, keeping the paging metadata.
pub fn map_page<S, T>(p: Page<S>, f: impl FnMut(S) -> T) -> Page<T> {
    Page {
        items: p.items.into_iter().map(f).collect(),
        page: p.page,
        page_size: p.page_size,
        total: p.total,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub owner_id: UserId,
    pub my_role: &'static str,
    pub created_at: DateTime<Utc>,
}

impl From<WorkspaceRecord> for WorkspaceDto {
    fn from(r: WorkspaceRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            slug: r.slug,
            description: r.description,
            logo_url: r.logo_url,
            owner_id: r.owner_id,
            my_role: r.my_role.as_str(),
            created_at: r.created_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDto {
    pub user_id: UserId,
    pub role: &'static str,
    pub joined_at: DateTime<Utc>,
    pub email: Option<String>,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<MemberView> for MemberDto {
    fn from(v: MemberView) -> Self {
        let (email, full_name, avatar_url) = match v.user {
            Some(u) => (Some(u.email), Some(u.full_name), u.avatar_url),
            None => (None, None, None),
        };
        Self {
            user_id: v.record.user_id,
            role: v.record.role.as_str(),
            joined_at: v.record.joined_at,
            email,
            full_name,
            avatar_url,
        }
    }
}
