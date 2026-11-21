//! sqlx implementation of [`InvitationRepo`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, MemberId, UserId, WorkspaceId};

use crate::application::invitations::{
    AcceptBy, AcceptOutcome, InvitationRecord, InvitationRepo, NewInvitation, PendingInvitation,
};
use crate::domain::{role_from_id, role_id};

pub struct PgInvitationRepo {
    pool: PgPool,
}

impl PgInvitationRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLS: &str = "i.id, i.workspace_id, i.email, i.role_id, i.invited_by, i.status, \
     i.expires_at, i.accepted_at, i.delivery_status, i.last_sent_at, i.sent_count, i.created_at";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    workspace_id: Uuid,
    email: String,
    role_id: Uuid,
    invited_by: Uuid,
    status: String,
    expires_at: DateTime<Utc>,
    accepted_at: Option<DateTime<Utc>>,
    delivery_status: String,
    last_sent_at: Option<DateTime<Utc>>,
    sent_count: i32,
    created_at: DateTime<Utc>,
}

impl Row {
    fn into_record(self) -> AppResult<InvitationRecord> {
        Ok(InvitationRecord {
            id: self.id,
            workspace_id: self.workspace_id.into(),
            email: self.email,
            role: role_from_id(self.role_id)
                .ok_or_else(|| AppError::internal("unknown invitation role id"))?,
            invited_by: self.invited_by.into(),
            status: self.status,
            expires_at: self.expires_at,
            accepted_at: self.accepted_at,
            delivery_status: self.delivery_status,
            last_sent_at: self.last_sent_at,
            sent_count: self.sent_count,
            created_at: self.created_at,
        })
    }
}

fn invalid() -> AppError {
    AppError::coded(
        404,
        "INVITATION_INVALID",
        "This invitation is invalid or no longer available.",
    )
}

fn not_found() -> AppError {
    AppError::not_found("invitation not found")
}

#[derive(sqlx::FromRow)]
struct AcceptRow {
    id: Uuid,
    workspace_id: Uuid,
    email: String,
    role_id: Uuid,
    membership_type: String,
    status: String,
    expires_at: DateTime<Utc>,
}

#[async_trait]
impl InvitationRepo for PgInvitationRepo {
    async fn insert(&self, new: NewInvitation) -> AppResult<InvitationRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE workspace.workspace_invitations SET status = 'expired' \
             WHERE workspace_id = $1 AND lower(email) = lower($2) \
             AND status = 'pending' AND expires_at <= $3",
        )
        .bind(new.workspace_id.as_uuid())
        .bind(&new.email)
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let sql = format!(
            "WITH i AS (INSERT INTO workspace.workspace_invitations \
             (id, workspace_id, email, role_id, invited_by, token_hash, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *) SELECT {COLS} FROM i"
        );
        let row = sqlx::query_as::<_, Row>(&sql)
            .bind(new.id)
            .bind(new.workspace_id.as_uuid())
            .bind(&new.email)
            .bind(role_id(new.role))
            .bind(new.invited_by.as_uuid())
            .bind(&new.token_hash)
            .bind(new.expires_at)
            .bind(new.now)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match map_sqlx_error(e) {
                AppError::Conflict { .. } => AppError::conflict(
                    "INVITATION_PENDING",
                    "A pending invitation already exists for this email.",
                ),
                other => other,
            })?;
        tx.commit().await.map_err(map_sqlx_error)?;
        row.into_record()
    }

    async fn list(&self, ws: WorkspaceId) -> AppResult<Vec<InvitationRecord>> {
        let sql = format!(
            "SELECT {COLS} FROM workspace.workspace_invitations i \
             WHERE i.workspace_id = $1 AND i.requested_by IS NULL AND i.status <> 'replaced' \
             ORDER BY i.created_at DESC LIMIT 500"
        );
        sqlx::query_as::<_, Row>(&sql)
            .bind(ws.as_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?
            .into_iter()
            .map(Row::into_record)
            .collect()
    }

    async fn revoke(&self, ws: WorkspaceId, id: Uuid) -> AppResult<()> {
        let done = sqlx::query(
            "UPDATE workspace.workspace_invitations SET status = 'revoked', token_hash = NULL \
             WHERE id = $1 AND workspace_id = $2 AND status IN ('pending', 'expired')",
        )
        .bind(id)
        .bind(ws.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        if done.rows_affected() > 0 {
            return Ok(());
        }
        let st: Option<(String,)> = sqlx::query_as(
            "SELECT status FROM workspace.workspace_invitations WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(ws.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        match st.as_ref().map(|(s,)| s.as_str()) {
            None => Err(not_found()),
            Some("accepted") => Err(AppError::conflict(
                "INVITATION_ACCEPTED",
                "The invitation has already been accepted.",
            )),
            Some(_) => Ok(()),
        }
    }

    async fn reissue(
        &self,
        ws: WorkspaceId,
        id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<InvitationRecord> {
        let sql = format!(
            "WITH i AS (UPDATE workspace.workspace_invitations SET token_hash = $3, \
             expires_at = $4, status = 'pending', delivery_status = 'NotSent' \
             WHERE id = $1 AND workspace_id = $2 AND status IN ('pending', 'expired') \
             RETURNING *) SELECT {COLS} FROM i"
        );
        let row = sqlx::query_as::<_, Row>(&sql)
            .bind(id)
            .bind(ws.as_uuid())
            .bind(token_hash)
            .bind(expires_at)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        if let Some(r) = row {
            return r.into_record();
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM workspace.workspace_invitations \
             WHERE id = $1 AND workspace_id = $2)",
        )
        .bind(id)
        .bind(ws.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        if exists {
            Err(AppError::conflict(
                "INVITATION_NOT_PENDING",
                "Only pending invitations can be resent.",
            ))
        } else {
            Err(not_found())
        }
    }

    async fn record_delivery(&self, id: Uuid, ok: bool, now: DateTime<Utc>) -> AppResult<()> {
        if ok {
            sqlx::query(
                "UPDATE workspace.workspace_invitations SET delivery_status = 'Sent', \
                 last_sent_at = $2, sent_count = sent_count + 1 WHERE id = $1",
            )
            .bind(id)
            .bind(now)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE workspace.workspace_invitations SET delivery_status = 'Failed' \
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&self.pool)
            .await
        }
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn pending_for_email(
        &self,
        email: &str,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<PendingInvitation>> {
        let sql = format!(
            "SELECT {COLS}, w.name AS workspace_name \
             FROM workspace.workspace_invitations i \
             JOIN workspace.workspaces w ON w.id = i.workspace_id \
             WHERE lower(i.email) = lower($1) AND i.status = 'pending' AND i.expires_at > $2 \
             AND i.token_hash IS NOT NULL AND w.deleted_at IS NULL AND w.is_active \
             ORDER BY i.created_at DESC LIMIT 200"
        );
        #[derive(sqlx::FromRow)]
        struct P {
            #[sqlx(flatten)]
            row: Row,
            workspace_name: String,
        }
        sqlx::query_as::<_, P>(&sql)
            .bind(email)
            .bind(now)
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?
            .into_iter()
            .map(|p| {
                Ok(PendingInvitation {
                    record: p.row.into_record()?,
                    workspace_name: p.workspace_name,
                })
            })
            .collect()
    }

    async fn accept(
        &self,
        by: AcceptBy,
        user: UserId,
        email: &str,
        now: DateTime<Utc>,
    ) -> AppResult<AcceptOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let base = "SELECT id, workspace_id, email, role_id, membership_type, status, expires_at \
                    FROM workspace.workspace_invitations WHERE ";
        let row: Option<AcceptRow> = match &by {
            AcceptBy::TokenHash(h) => {
                sqlx::query_as(&format!("{base}token_hash = $1 FOR UPDATE"))
                    .bind(h)
                    .fetch_optional(&mut *tx)
                    .await
            }
            AcceptBy::Id(id) => {
                sqlx::query_as(&format!("{base}id = $1 FOR UPDATE"))
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
            }
        }
        .map_err(map_sqlx_error)?;
        let inv = row.ok_or_else(invalid)?;
        if !inv.email.eq_ignore_ascii_case(email) {
            // Same answer as unknown for id-based access would leak existence; token holders
            // get an explicit mismatch (BR-36).
            return Err(match by {
                AcceptBy::TokenHash(_) => AppError::coded(
                    403,
                    "INVITATION_EMAIL_MISMATCH",
                    "This invitation was sent to a different email address.",
                ),
                AcceptBy::Id(_) => invalid(),
            });
        }
        let ws: (String, bool) = sqlx::query_as(
            "SELECT name, (deleted_at IS NULL AND is_active) FROM workspace.workspaces WHERE id = $1",
        )
        .bind(inv.workspace_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .ok_or_else(invalid)?;
        if !ws.1 {
            return Err(invalid());
        }
        let role = role_from_id(inv.role_id)
            .ok_or_else(|| AppError::internal("unknown invitation role id"))?;
        let outcome = AcceptOutcome {
            workspace_id: inv.workspace_id.into(),
            workspace_name: ws.0,
            role,
        };
        match inv.status.as_str() {
            "accepted" => {
                // Idempotent: still an active member -> success.
                let member: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM workspace.workspace_members \
                     WHERE workspace_id = $1 AND user_id = $2 AND status = 'active')",
                )
                .bind(inv.workspace_id)
                .bind(user.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                return if member { Ok(outcome) } else { Err(invalid()) };
            }
            "pending" => {}
            _ => return Err(invalid()),
        }
        if inv.expires_at <= now {
            return Err(AppError::coded(
                410,
                "INVITATION_EXPIRED",
                "This invitation has expired.",
            ));
        }
        sqlx::query(
            "INSERT INTO workspace.workspace_members \
             (id, workspace_id, user_id, role_id, membership_type, joined_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (workspace_id, user_id) DO UPDATE SET \
               role_id = CASE WHEN workspace.workspace_members.status = 'active' \
                              THEN workspace.workspace_members.role_id ELSE EXCLUDED.role_id END, \
               joined_at = CASE WHEN workspace.workspace_members.status = 'active' \
                                THEN workspace.workspace_members.joined_at ELSE EXCLUDED.joined_at END, \
               status = 'active', removed_at = NULL, removed_by = NULL",
        )
        .bind(MemberId::new().as_uuid())
        .bind(inv.workspace_id)
        .bind(user.as_uuid())
        .bind(inv.role_id)
        .bind(&inv.membership_type)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE workspace.workspace_invitations SET status = 'accepted', accepted_at = $2 \
             WHERE id = $1",
        )
        .bind(inv.id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(outcome)
    }
}
