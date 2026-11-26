use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use warptalk_kernel::{Page, UserId, WorkspaceId};

use crate::application::invitations::{AcceptOutcome, InvitationRecord, InvitationView};
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
    pub membership_type: String,
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
            membership_type: v.record.membership_type,
            email,
            full_name,
            avatar_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteRequest {
    pub email: String,
    pub role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptRequest {
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitationDto {
    pub id: uuid::Uuid,
    pub workspace_id: WorkspaceId,
    pub email: String,
    pub role: &'static str,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub delivery_status: String,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub sent_count: i32,
    pub invited_by: UserId,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inviter_email: Option<String>,
}

impl From<InvitationRecord> for InvitationDto {
    fn from(r: InvitationRecord) -> Self {
        Self {
            id: r.id,
            workspace_id: r.workspace_id,
            email: r.email,
            role: r.role.as_str(),
            status: r.status,
            expires_at: r.expires_at,
            accepted_at: r.accepted_at,
            delivery_status: r.delivery_status,
            last_sent_at: r.last_sent_at,
            sent_count: r.sent_count,
            invited_by: r.invited_by,
            created_at: r.created_at,
            workspace_name: None,
            inviter_email: None,
        }
    }
}

impl From<InvitationView> for InvitationDto {
    fn from(v: InvitationView) -> Self {
        let mut d = Self::from(v.record);
        d.workspace_name = v.workspace_name;
        d.inviter_email = v.inviter_email;
        d
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptDto {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub role: &'static str,
}

impl From<AcceptOutcome> for AcceptDto {
    fn from(o: AcceptOutcome) -> Self {
        Self {
            workspace_id: o.workspace_id,
            workspace_name: o.workspace_name,
            role: o.role.as_str(),
        }
    }
}

/// `GET /workspaces/{id}/entitlements`. `isKnown` is false until billing's first snapshot is
/// projected; the client must not render defaults in its place.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementsDto {
    pub is_known: bool,
    pub plan_slug: Option<String>,
    pub has_active_subscription: bool,
    pub resolved_at: Option<DateTime<Utc>>,
    pub entitlements: Vec<EntitlementDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementDto {
    pub key: String,
    pub kind: crate::domain::entitlements::EntitlementKind,
    pub value: String,
    pub source: String,
    pub ceiling: Option<String>,
    pub ceiling_source: Option<String>,
}

impl From<Option<crate::domain::entitlements::EntitlementSnapshot>> for EntitlementsDto {
    fn from(s: Option<crate::domain::entitlements::EntitlementSnapshot>) -> Self {
        match s {
            None => Self {
                is_known: false,
                plan_slug: None,
                has_active_subscription: false,
                resolved_at: None,
                entitlements: Vec::new(),
            },
            Some(s) => Self {
                is_known: true,
                plan_slug: s.plan_slug,
                has_active_subscription: s.has_active_subscription,
                resolved_at: Some(s.resolved_at),
                entitlements: s
                    .entitlements
                    .into_iter()
                    .map(|(key, v)| EntitlementDto {
                        key,
                        kind: v.kind,
                        value: v.value,
                        source: v.source,
                        ceiling: v.ceiling,
                        ceiling_source: v.ceiling_source,
                    })
                    .collect(),
            },
        }
    }
}
