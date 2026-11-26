//! Admin HTTP for the runtime settings store: `/settings/{namespace}`.

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, RoomId, UserId, WorkspaceId, WorkspaceRole};
use warptalk_room::RoomQueries;
use warptalk_settings::{
    AiProvidersSettings, NS_AI_PROVIDERS, NS_PIPELINE, NS_PLATFORM_STATUS, NS_RAG, NS_SUGGESTION,
    PipelineSettings, PlatformStatusSettings, RagSettings, Scope, ScopeChain, SettingsStore,
    SuggestionSettings,
};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};
use warptalk_workspace::WorkspaceQueries;

#[derive(Clone)]
pub struct SettingsHttpState {
    pub store: SettingsStore,
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub workspaces: WorkspaceQueries,
    pub rooms: RoomQueries,
}

impl FromRef<SettingsHttpState> for JwtKeys {
    fn from_ref(s: &SettingsHttpState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: SettingsHttpState) -> Router {
    Router::new()
        .route(
            "/settings/{namespace}",
            get(get_settings).put(put_settings).delete(delete_settings),
        )
        .route("/platform/status", get(platform_status))
        .with_state(state)
}

/// Public (no auth): what every visitor's client shows in the platform banner. Read from the
/// platform scope of `platform_status`, which only a system admin can write.
async fn platform_status(State(st): State<SettingsHttpState>) -> AppResult<Json<Value>> {
    let s: PlatformStatusSettings = st
        .store
        .get(NS_PLATFORM_STATUS, ScopeChain::platform())
        .await?;
    let message = s
        .maintenance
        .message
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    Ok(Json(json!({
        "maintenance": { "enabled": s.maintenance.enabled, "message": message },
        "supportEmail": s.support_email.trim(),
        // This backend has no Google OAuth flow; the client hides the button while false.
        "googleSignInEnabled": false,
    })))
}

/// Namespaces that only exist at platform scope (no workspace/room overrides).
fn platform_only(ns: &str, scope: Scope) -> AppResult<()> {
    if ns == NS_PLATFORM_STATUS && scope != Scope::Platform {
        return Err(AppError::validation(
            "SETTINGS_SCOPE_INVALID",
            format!("`{ns}` is set at platform scope only"),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeQuery {
    scope: Option<String>,
    id: Option<Uuid>,
    #[serde(default)]
    effective: bool,
    workspace_id: Option<Uuid>,
    room_id: Option<Uuid>,
}

fn known_namespace(ns: &str) -> AppResult<()> {
    if [
        NS_PIPELINE,
        NS_SUGGESTION,
        NS_RAG,
        NS_AI_PROVIDERS,
        NS_PLATFORM_STATUS,
    ]
    .contains(&ns)
    {
        Ok(())
    } else {
        Err(AppError::not_found(format!("unknown namespace `{ns}`")))
    }
}

fn parse_scope(q: &ScopeQuery) -> AppResult<Scope> {
    let need_id = || {
        q.id.ok_or_else(|| {
            AppError::validation("SETTINGS_SCOPE_INVALID", "`id` is required for this scope")
        })
    };
    match q.scope.as_deref() {
        Some("platform") => Ok(Scope::Platform),
        Some("workspace") => Ok(Scope::Workspace(WorkspaceId::from_uuid(need_id()?))),
        Some("room") => Ok(Scope::Room(RoomId::from_uuid(need_id()?))),
        _ => Err(AppError::validation(
            "SETTINGS_SCOPE_INVALID",
            "`scope` must be platform, workspace or room",
        )),
    }
}

fn is_admin(role: Option<WorkspaceRole>) -> bool {
    matches!(role, Some(WorkspaceRole::Owner | WorkspaceRole::Admin))
}

/// Write access: platform -> system admin; workspace -> OWNER/ADMIN; room -> host or
/// workspace OWNER/ADMIN.
async fn authorize_admin(
    st: &SettingsHttpState,
    user: UserId,
    system_admin: bool,
    scope: Scope,
) -> AppResult<()> {
    let denied = || AppError::forbidden("insufficient permissions for this settings scope");
    match scope {
        Scope::Platform => {
            if system_admin {
                Ok(())
            } else {
                Err(AppError::forbidden("system admin required"))
            }
        }
        Scope::Workspace(ws) => {
            if system_admin || is_admin(st.workspaces.member_role(ws, user).await?) {
                Ok(())
            } else {
                Err(denied())
            }
        }
        Scope::Room(room) => {
            let summary = st
                .rooms
                .room_summary(room)
                .await?
                .ok_or_else(|| AppError::not_found("room not found"))?;
            if system_admin
                || user == summary.host_id
                || user == summary.effective_host_id
                || is_admin(
                    st.workspaces
                        .member_role(summary.workspace_id, user)
                        .await?,
                )
            {
                Ok(())
            } else {
                Err(denied())
            }
        }
    }
}

/// Any member of the workspace (resolved from the room when only `roomId` is given).
async fn effective_chain(
    st: &SettingsHttpState,
    user: UserId,
    q: &ScopeQuery,
) -> AppResult<ScopeChain> {
    let mut workspace = q.workspace_id.map(WorkspaceId::from_uuid);
    let room = q.room_id.map(RoomId::from_uuid);
    if let Some(room) = room {
        let summary = st
            .rooms
            .room_summary(room)
            .await?
            .ok_or_else(|| AppError::not_found("room not found"))?;
        if workspace.is_some_and(|w| w != summary.workspace_id) {
            return Err(AppError::validation(
                "SETTINGS_SCOPE_INVALID",
                "room does not belong to workspaceId",
            ));
        }
        workspace = Some(summary.workspace_id);
    }
    let Some(ws) = workspace else {
        return Ok(ScopeChain::platform());
    };
    if st.workspaces.member_role(ws, user).await?.is_none() {
        return Err(AppError::forbidden("not a member of this workspace"));
    }
    Ok(match room {
        Some(r) => ScopeChain::room(ws, r),
        None => ScopeChain::workspace(ws),
    })
}

async fn effective_value(store: &SettingsStore, ns: &str, chain: ScopeChain) -> AppResult<Value> {
    fn ser<T: serde::Serialize>(t: T) -> AppResult<Value> {
        serde_json::to_value(t).map_err(|e| AppError::internal(e.to_string()))
    }
    match ns {
        NS_PIPELINE => ser(store.get::<PipelineSettings>(ns, chain).await?),
        NS_SUGGESTION => ser(store.get::<SuggestionSettings>(ns, chain).await?),
        NS_RAG => ser(store.get::<RagSettings>(ns, chain).await?),
        NS_AI_PROVIDERS => ser(store.get::<AiProvidersSettings>(ns, chain).await?),
        NS_PLATFORM_STATUS => ser(store
            .get::<PlatformStatusSettings>(ns, ScopeChain::platform())
            .await?),
        _ => Err(AppError::not_found(format!("unknown namespace `{ns}`"))),
    }
}

fn scope_json(scope: Scope) -> Value {
    serde_json::to_value(scope).unwrap_or(Value::Null)
}

async fn get_settings(
    State(st): State<SettingsHttpState>,
    AuthUser(claims): AuthUser,
    Path(ns): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> AppResult<Json<Value>> {
    known_namespace(&ns)?;
    if q.effective {
        let chain = effective_chain(&st, claims.sub, &q).await?;
        let value = effective_value(&st.store, &ns, chain).await?;
        return Ok(Json(
            json!({ "namespace": ns, "effective": true, "value": value }),
        ));
    }
    let scope = parse_scope(&q)?;
    authorize_admin(&st, claims.sub, claims.sa, scope).await?;
    let row: Option<(Value, i64)> = sqlx::query_as(
        "SELECT value, version FROM public.settings_entries \
         WHERE scope_kind = $1 AND scope_id = $2 AND namespace = $3",
    )
    .bind(scope_kind(scope))
    .bind(scope_id(scope))
    .bind(&ns)
    .fetch_optional(&st.pool)
    .await
    .map_err(warptalk_db::map_sqlx_error)?;
    let (value, version) = match row {
        Some((v, ver)) => (v, Some(ver)),
        None => (Value::Null, None),
    };
    Ok(Json(json!({
        "namespace": ns, "scope": scope_json(scope), "value": value, "version": version
    })))
}

fn scope_kind(scope: Scope) -> &'static str {
    match scope {
        Scope::Platform => "platform",
        Scope::Workspace(_) => "workspace",
        Scope::Room(_) => "room",
    }
}

fn scope_id(scope: Scope) -> Uuid {
    match scope {
        Scope::Platform => Uuid::nil(),
        Scope::Workspace(w) => w.as_uuid(),
        Scope::Room(r) => r.as_uuid(),
    }
}

async fn put_settings(
    State(st): State<SettingsHttpState>,
    AuthUser(claims): AuthUser,
    Path(ns): Path<String>,
    Query(q): Query<ScopeQuery>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    known_namespace(&ns)?;
    let scope = parse_scope(&q)?;
    platform_only(&ns, scope)?;
    authorize_admin(&st, claims.sub, claims.sa, scope).await?;
    let out = st.store.put(scope, &ns, body, Some(claims.sub)).await?;
    Ok(Json(json!({
        "namespace": ns, "scope": scope_json(scope), "version": out.version
    })))
}

async fn delete_settings(
    State(st): State<SettingsHttpState>,
    AuthUser(claims): AuthUser,
    Path(ns): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> AppResult<StatusCode> {
    known_namespace(&ns)?;
    let scope = parse_scope(&q)?;
    platform_only(&ns, scope)?;
    authorize_admin(&st, claims.sub, claims.sa, scope).await?;
    st.store.delete(scope, &ns, Some(claims.sub)).await?;
    Ok(StatusCode::NO_CONTENT)
}
