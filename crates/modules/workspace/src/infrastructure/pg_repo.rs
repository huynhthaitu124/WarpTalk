//! sqlx implementation of [`WorkspaceRepo`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{
    AppError, AppResult, MemberId, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole,
};

use crate::application::ports::{MemberRecord, NewWorkspace, WorkspaceRecord, WorkspaceRepo};
use crate::domain::{self, WorkspaceSettings, role_from_id, role_id};

pub struct PgWorkspaceRepo {
    pool: PgPool,
}

impl PgWorkspaceRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct WsRow {
    id: Uuid,
    name: String,
    slug: String,
    owner_id: Uuid,
    logo_url: Option<String>,
    settings: serde_json::Value,
    created_at: DateTime<Utc>,
    role_id: Uuid,
}

fn to_role(id: Uuid) -> AppResult<WorkspaceRole> {
    role_from_id(id).ok_or_else(|| AppError::internal(format!("unknown role id {id}")))
}

impl WsRow {
    fn into_record(self) -> AppResult<WorkspaceRecord> {
        Ok(WorkspaceRecord {
            id: self.id.into(),
            name: self.name,
            slug: self.slug,
            description: self
                .settings
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            logo_url: self.logo_url,
            owner_id: self.owner_id.into(),
            my_role: to_role(self.role_id)?,
            created_at: self.created_at,
        })
    }
}

const WS_COLS: &str =
    "w.id, w.name, w.slug, w.owner_id, w.logo_url, w.settings, w.created_at, m.role_id";
const ACTIVE_MEMBER: &str = "m.status = 'active' AND m.removed_at IS NULL \
     AND w.deleted_at IS NULL AND w.is_active";

/// Role of an active member of an active workspace.
async fn role_of(
    conn: &mut PgConnection,
    ws: WorkspaceId,
    user: UserId,
) -> AppResult<Option<WorkspaceRole>> {
    let sql = format!(
        "SELECT m.role_id FROM workspace.workspace_members m \
         JOIN workspace.workspaces w ON w.id = m.workspace_id \
         WHERE m.workspace_id = $1 AND m.user_id = $2 AND {ACTIVE_MEMBER}"
    );
    let row: Option<(Uuid,)> = sqlx::query_as(&sql)
        .bind(ws.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(conn)
        .await
        .map_err(map_sqlx_error)?;
    row.map(|(r,)| to_role(r)).transpose()
}

/// Member role or `NotFound` (never leaks existence to non-members).
async fn require_member(
    conn: &mut PgConnection,
    ws: WorkspaceId,
    user: UserId,
) -> AppResult<WorkspaceRole> {
    role_of(conn, ws, user)
        .await?
        .ok_or_else(|| AppError::not_found("workspace not found"))
}

/// Locks an active workspace row for the duration of the transaction.
async fn lock_workspace(conn: &mut PgConnection, ws: WorkspaceId) -> AppResult<()> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM workspace.workspaces \
         WHERE id = $1 AND deleted_at IS NULL AND is_active FOR UPDATE",
    )
    .bind(ws.as_uuid())
    .fetch_optional(conn)
    .await
    .map_err(map_sqlx_error)?;
    row.map(|_| ())
        .ok_or_else(|| AppError::not_found("workspace not found"))
}

async fn set_role(
    conn: &mut PgConnection,
    ws: WorkspaceId,
    user: UserId,
    role: WorkspaceRole,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE workspace.workspace_members SET role_id = $3 \
         WHERE workspace_id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(ws.as_uuid())
    .bind(user.as_uuid())
    .bind(role_id(role))
    .execute(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn mark_removed(
    conn: &mut PgConnection,
    ws: WorkspaceId,
    user: UserId,
    by: UserId,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE workspace.workspace_members \
         SET status = 'removed', removed_at = $3, removed_by = $4 \
         WHERE workspace_id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(ws.as_uuid())
    .bind(user.as_uuid())
    .bind(now)
    .bind(by.as_uuid())
    .execute(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

#[async_trait]
impl WorkspaceRepo for PgWorkspaceRepo {
    async fn create(&self, new: NewWorkspace) -> AppResult<WorkspaceRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let settings = match &new.description {
            Some(d) => serde_json::json!({ "description": d }),
            None => serde_json::json!({}),
        };
        sqlx::query(
            "INSERT INTO workspace.workspaces \
             (id, name, slug, owner_id, settings, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $4)",
        )
        .bind(new.id.as_uuid())
        .bind(&new.name)
        .bind(&new.slug)
        .bind(new.owner.as_uuid())
        .bind(&settings)
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO workspace.workspace_members \
             (id, workspace_id, user_id, role_id, joined_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(MemberId::new().as_uuid())
        .bind(new.id.as_uuid())
        .bind(new.owner.as_uuid())
        .bind(role_id(WorkspaceRole::Owner))
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(WorkspaceRecord {
            id: new.id,
            name: new.name,
            slug: new.slug,
            description: new.description,
            logo_url: None,
            owner_id: new.owner,
            my_role: WorkspaceRole::Owner,
            created_at: new.now,
        })
    }

    async fn list_for_user(
        &self,
        user: UserId,
        search: Option<&str>,
        page: &PageRequest,
    ) -> AppResult<Page<WorkspaceRecord>> {
        let filter = format!(
            "FROM workspace.workspace_members m \
             JOIN workspace.workspaces w ON w.id = m.workspace_id \
             WHERE m.user_id = $1 AND {ACTIVE_MEMBER} \
             AND ($2::text IS NULL OR position(lower($2::text) in lower(w.name)) > 0)"
        );
        let (total,): (i64,) = sqlx::query_as(&format!("SELECT count(*) {filter}"))
            .bind(user.as_uuid())
            .bind(search)
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        let rows: Vec<WsRow> = sqlx::query_as(&format!(
            "SELECT {WS_COLS} {filter} ORDER BY w.created_at DESC, w.id LIMIT $3 OFFSET $4"
        ))
        .bind(user.as_uuid())
        .bind(search)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let items = rows
            .into_iter()
            .map(WsRow::into_record)
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Page::new(items, page, total))
    }

    async fn get_for_member(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRecord> {
        let row: Option<WsRow> = sqlx::query_as(&format!(
            "SELECT {WS_COLS} FROM workspace.workspace_members m \
             JOIN workspace.workspaces w ON w.id = m.workspace_id \
             WHERE m.workspace_id = $1 AND m.user_id = $2 AND {ACTIVE_MEMBER}"
        ))
        .bind(ws.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.ok_or_else(|| AppError::not_found("workspace not found"))?
            .into_record()
    }

    async fn delete(&self, ws: WorkspaceId, actor: UserId, now: DateTime<Utc>) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let role = require_member(&mut tx, ws, actor).await?;
        domain::authorize_delete(role)?;
        sqlx::query(
            "UPDATE workspace.workspaces SET deleted_at = $2, deleted_by = $3, \
             is_active = false, updated_at = $2, updated_by = $3 WHERE id = $1",
        )
        .bind(ws.as_uuid())
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn get_settings(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceSettings> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        require_member(&mut conn, ws, user).await?;
        let (v,): (serde_json::Value,) =
            sqlx::query_as("SELECT settings FROM workspace.workspaces WHERE id = $1")
                .bind(ws.as_uuid())
                .fetch_one(&mut *conn)
                .await
                .map_err(map_sqlx_error)?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    async fn update_settings(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        settings: &WorkspaceSettings,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let patch = serde_json::to_value(settings)
            .map_err(|e| AppError::internal(format!("serialize settings: {e}")))?;
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let role = require_member(&mut tx, ws, actor).await?;
        domain::authorize_update_settings(role)?;
        sqlx::query(
            "UPDATE workspace.workspaces SET settings = settings || $2::jsonb, \
             updated_at = $3, updated_by = $4 WHERE id = $1",
        )
        .bind(ws.as_uuid())
        .bind(&patch)
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn list_members(
        &self,
        ws: WorkspaceId,
        user: UserId,
        page: &PageRequest,
    ) -> AppResult<Page<MemberRecord>> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        require_member(&mut conn, ws, user).await?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM workspace.workspace_members \
             WHERE workspace_id = $1 AND status = 'active' AND removed_at IS NULL",
        )
        .bind(ws.as_uuid())
        .fetch_one(&mut *conn)
        .await
        .map_err(map_sqlx_error)?;
        let rows: Vec<(Uuid, Uuid, DateTime<Utc>)> = sqlx::query_as(
            "SELECT user_id, role_id, joined_at FROM workspace.workspace_members \
             WHERE workspace_id = $1 AND status = 'active' AND removed_at IS NULL \
             ORDER BY joined_at, id LIMIT $2 OFFSET $3",
        )
        .bind(ws.as_uuid())
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(&mut *conn)
        .await
        .map_err(map_sqlx_error)?;
        let items = rows
            .into_iter()
            .map(|(u, r, joined_at)| {
                Ok(MemberRecord {
                    user_id: u.into(),
                    role: to_role(r)?,
                    joined_at,
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Page::new(items, page, total))
    }

    async fn change_role(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
        role: WorkspaceRole,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let actor_role = require_member(&mut tx, ws, actor).await?;
        domain::authorize_change_role((actor, actor_role), target, role)?;
        require_member(&mut tx, ws, target).await?;
        set_role(&mut tx, ws, target, role).await?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn remove_member(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        target: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let actor_role = require_member(&mut tx, ws, actor).await?;
        let target_role = role_of(&mut tx, ws, target)
            .await?
            .ok_or_else(|| AppError::not_found("member not found"))?;
        domain::authorize_remove(actor_role, target_role)?;
        mark_removed(&mut tx, ws, target, actor, now).await?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn leave(&self, ws: WorkspaceId, actor: UserId, now: DateTime<Utc>) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let role = require_member(&mut tx, ws, actor).await?;
        domain::authorize_leave(role)?;
        mark_removed(&mut tx, ws, actor, actor, now).await?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn transfer_ownership(
        &self,
        ws: WorkspaceId,
        actor: UserId,
        new_owner: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        lock_workspace(&mut tx, ws).await?;
        let actor_role = require_member(&mut tx, ws, actor).await?;
        domain::authorize_transfer((actor, actor_role), new_owner)?;
        if role_of(&mut tx, ws, new_owner).await?.is_none() {
            return Err(AppError::validation(
                "NEW_OWNER_NOT_MEMBER",
                "new owner must be an active member of the workspace",
            ));
        }
        set_role(&mut tx, ws, new_owner, WorkspaceRole::Owner).await?;
        set_role(&mut tx, ws, actor, WorkspaceRole::Admin).await?;
        sqlx::query(
            "UPDATE workspace.workspaces SET owner_id = $2, updated_at = $3, updated_by = $4 \
             WHERE id = $1",
        )
        .bind(ws.as_uuid())
        .bind(new_owner.as_uuid())
        .bind(now)
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        role_of(&mut conn, ws, user).await
    }

    async fn is_active(&self, ws: WorkspaceId) -> AppResult<bool> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspace.workspaces \
             WHERE id = $1 AND deleted_at IS NULL AND is_active",
        )
        .bind(ws.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(row.is_some())
    }
}
