//! Workspace invitations: invite, list, revoke, resend, accept (BR-17, BR-33, BR-34, BR-36).

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, UserId, WorkspaceId, WorkspaceRole};

use super::ports::{UserDirectory, WorkspaceRepo};

/// Default invitation lifetime (BR-33).
pub const DEFAULT_INVITATION_TTL_DAYS: i64 = 7;

#[derive(Debug, Clone)]
pub struct InvitationRecord {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub email: String,
    pub role: WorkspaceRole,
    pub invited_by: UserId,
    /// `pending`, `accepted`, `revoked` or `expired`.
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub delivery_status: String,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub sent_count: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PendingInvitation {
    pub record: InvitationRecord,
    pub workspace_name: String,
}

#[derive(Debug, Clone)]
pub struct NewInvitation {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub email: String,
    pub role: WorkspaceRole,
    pub invited_by: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum AcceptBy {
    TokenHash(String),
    Id(Uuid),
}

#[derive(Debug, Clone)]
pub struct AcceptOutcome {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub role: WorkspaceRole,
}

/// What the mailer needs to deliver an invitation. `token` is the RAW secret: never log it.
pub struct InvitationMail<'a> {
    pub to: &'a str,
    pub workspace_name: &'a str,
    pub inviter_name: &'a str,
    pub role: WorkspaceRole,
    pub token: &'a str,
    pub expires_at: DateTime<Utc>,
}

/// Delivers invitation emails. Implementations must not log the token.
#[async_trait]
pub trait InvitationMailer: Send + Sync {
    async fn send_invitation(&self, mail: &InvitationMail<'_>) -> AppResult<()>;
}

#[async_trait]
pub trait InvitationRepo: Send + Sync {
    /// Expires stale pending rows for the email, then inserts. Pending duplicate -> Conflict
    /// `INVITATION_PENDING`.
    async fn insert(&self, new: NewInvitation) -> AppResult<InvitationRecord>;
    async fn list(&self, ws: WorkspaceId) -> AppResult<Vec<InvitationRecord>>;
    /// Pending -> revoked. Accepted -> Conflict. Unknown -> NotFound. Revoked: idempotent.
    async fn revoke(&self, ws: WorkspaceId, id: Uuid) -> AppResult<()>;
    /// Replaces the token (old one stops working) and restarts the expiry window.
    /// Accepted/revoked -> Conflict. Unknown -> NotFound.
    async fn reissue(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<InvitationRecord>;
    async fn record_delivery(&self, id: Uuid, ok: bool, now: DateTime<Utc>) -> AppResult<()>;
    async fn pending_for_email(
        &self,
        email: &str,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<PendingInvitation>>;
    /// Identity-bound, idempotent acceptance (see service for error codes).
    async fn accept(
        &self,
        by: AcceptBy,
        user: UserId,
        email: &str,
        now: DateTime<Utc>,
    ) -> AppResult<AcceptOutcome>;
}

pub struct InvitationView {
    pub record: InvitationRecord,
    pub inviter_email: Option<String>,
    pub workspace_name: Option<String>,
}

pub struct InvitationService {
    repo: Arc<dyn InvitationRepo>,
    workspaces: Arc<dyn WorkspaceRepo>,
    users: Arc<dyn UserDirectory>,
    mailer: Arc<dyn InvitationMailer>,
    clock: Arc<dyn Clock>,
    ttl: Duration,
}

pub fn generate_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

pub fn hash_token(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn normalize_email(email: &str) -> AppResult<String> {
    let e = email.trim().to_lowercase();
    let ok = e.len() <= 320
        && e.split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
        && !e.chars().any(|c| c.is_whitespace());
    if ok {
        Ok(e)
    } else {
        Err(AppError::validation(
            "INVALID_EMAIL",
            "a valid email address is required",
        ))
    }
}

impl InvitationService {
    pub fn new(
        repo: Arc<dyn InvitationRepo>,
        workspaces: Arc<dyn WorkspaceRepo>,
        users: Arc<dyn UserDirectory>,
        mailer: Arc<dyn InvitationMailer>,
        clock: Arc<dyn Clock>,
        ttl_days: i64,
    ) -> Self {
        Self {
            repo,
            workspaces,
            users,
            mailer,
            clock,
            ttl: Duration::days(if ttl_days > 0 {
                ttl_days
            } else {
                DEFAULT_INVITATION_TTL_DAYS
            }),
        }
    }

    /// Non-members get NotFound, plain members Forbidden.
    async fn manager_role(&self, ws: WorkspaceId, actor: UserId) -> AppResult<WorkspaceRole> {
        match self.workspaces.member_role(ws, actor).await? {
            None => Err(AppError::not_found("workspace not found")),
            Some(WorkspaceRole::Member) => Err(AppError::forbidden(
                "Only Owner or Admin can manage invitations",
            )),
            Some(r) => Ok(r),
        }
    }

    async fn email_of(&self, user: UserId) -> AppResult<(String, String)> {
        self.users
            .summaries(&[user])
            .await?
            .remove(&user)
            .map(|u| (u.email, u.full_name))
            .ok_or_else(|| AppError::unauthorized("unknown user"))
    }

    async fn deliver(&self, rec: &InvitationRecord, token: &str) -> AppResult<InvitationRecord> {
        let ws_name = self
            .workspaces
            .get_for_member(rec.workspace_id, rec.invited_by)
            .await
            .map(|w| w.name)
            .unwrap_or_default();
        let inviter = self
            .users
            .summaries(&[rec.invited_by])
            .await
            .ok()
            .and_then(|mut m| m.remove(&rec.invited_by));
        let inviter_name = inviter
            .map(|u| {
                if u.full_name.is_empty() {
                    u.email
                } else {
                    u.full_name
                }
            })
            .unwrap_or_default();
        let now = self.clock.now();
        let sent = self
            .mailer
            .send_invitation(&InvitationMail {
                to: &rec.email,
                workspace_name: &ws_name,
                inviter_name: &inviter_name,
                role: rec.role,
                token,
                expires_at: rec.expires_at,
            })
            .await;
        if let Err(e) = &sent {
            tracing::warn!(invitation = %rec.id, error = %e, "invitation delivery failed");
        }
        self.repo.record_delivery(rec.id, sent.is_ok(), now).await?;
        let mut out = rec.clone();
        if sent.is_ok() {
            out.delivery_status = "Sent".into();
            out.last_sent_at = Some(now);
            out.sent_count += 1;
        } else {
            out.delivery_status = "Failed".into();
        }
        Ok(out)
    }

    pub async fn invite(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        email: &str,
        role: &str,
    ) -> AppResult<InvitationRecord> {
        let role: WorkspaceRole =
            role.trim().to_uppercase().parse().map_err(|_| {
                AppError::validation("INVALID_ROLE", "role must be ADMIN or MEMBER")
            })?;
        if role == WorkspaceRole::Owner {
            return Err(AppError::validation(
                "INVALID_ROLE",
                "The Owner role cannot be granted by invitation",
            ));
        }
        let email = normalize_email(email)?;
        let actor_role = self.manager_role(ws, actor).await?;
        if role == WorkspaceRole::Admin && actor_role != WorkspaceRole::Owner {
            return Err(AppError::forbidden("Only the Owner can invite an Admin"));
        }
        let now = self.clock.now();
        let token = generate_token();
        let rec = self
            .repo
            .insert(NewInvitation {
                id: Uuid::now_v7(),
                workspace_id: ws,
                email,
                role,
                invited_by: actor,
                token_hash: hash_token(&token),
                expires_at: now + self.ttl,
                now,
            })
            .await?;
        self.deliver(&rec, &token).await
    }

    pub async fn list(&self, ws: WorkspaceId, actor: UserId) -> AppResult<Vec<InvitationRecord>> {
        self.manager_role(ws, actor).await?;
        self.repo.list(ws).await
    }

    pub async fn revoke(&self, ws: WorkspaceId, actor: UserId, id: Uuid) -> AppResult<()> {
        self.manager_role(ws, actor).await?;
        self.repo.revoke(ws, id).await
    }

    pub async fn resend(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        id: Uuid,
    ) -> AppResult<InvitationRecord> {
        self.manager_role(ws, actor).await?;
        let token = generate_token();
        let rec = self
            .repo
            .reissue(ws, id, &hash_token(&token), self.clock.now() + self.ttl)
            .await?;
        self.deliver(&rec, &token).await
    }

    /// Accepts by raw token or id. The user's email must match the invited email
    /// (case-insensitive, BR-36). Re-accepting an accepted invitation is a no-op success.
    pub async fn accept(&self, user: UserId, by: AcceptBy) -> AppResult<AcceptOutcome> {
        let (email, _) = self.email_of(user).await?;
        self.repo.accept(by, user, &email, self.clock.now()).await
    }

    pub async fn accept_token(&self, user: UserId, token: &str) -> AppResult<AcceptOutcome> {
        let t = token.trim();
        if t.is_empty() {
            return Err(AppError::validation("INVALID_TOKEN", "token is required"));
        }
        self.accept(user, AcceptBy::TokenHash(hash_token(t))).await
    }

    pub async fn pending_for_user(&self, user: UserId) -> AppResult<Vec<InvitationView>> {
        let (email, _) = self.email_of(user).await?;
        let list = self
            .repo
            .pending_for_email(&email, self.clock.now())
            .await?;
        let ids: Vec<UserId> = list.iter().map(|p| p.record.invited_by).collect();
        let mut dir = self.users.summaries(&ids).await?;
        Ok(list
            .into_iter()
            .map(|p| InvitationView {
                inviter_email: dir.get_mut(&p.record.invited_by).map(|u| u.email.clone()),
                workspace_name: Some(p.workspace_name),
                record: p.record,
            })
            .collect())
    }
}
