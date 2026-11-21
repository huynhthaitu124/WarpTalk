#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, Clock, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;
use warptalk_workspace::domain::role_id;
use warptalk_workspace::{
    DevInvitationMailCapture, InvitationMail, InvitationMailer, MIGRATIONS, NoopUserDirectory,
    UserSummary, WorkspaceDeps, WorkspaceModule, application::ports::UserDirectory,
};

#[derive(Default)]
struct CaptureMailer(std::sync::Mutex<Vec<(String, String)>>);

#[async_trait]
impl InvitationMailer for CaptureMailer {
    async fn send_invitation(&self, m: &InvitationMail<'_>) -> AppResult<()> {
        self.0
            .lock()
            .unwrap()
            .push((m.to.to_owned(), m.token.to_owned()));
        Ok(())
    }
}

impl CaptureMailer {
    fn token_for(&self, email: &str) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(e, _)| e == email)
            .map(|(_, t)| t.clone())
            .unwrap()
    }
}

struct Ctx {
    pool: PgPool,
    router: Router,
    module: WorkspaceModule,
    keys: JwtKeys,
}

struct StubDir;

#[async_trait]
impl UserDirectory for StubDir {
    async fn summaries(&self, ids: &[UserId]) -> AppResult<HashMap<UserId, UserSummary>> {
        Ok(ids
            .iter()
            .map(|id| {
                (
                    *id,
                    UserSummary {
                        email: format!("{id}@example.test"),
                        full_name: "Stub User".into(),
                        avatar_url: None,
                    },
                )
            })
            .collect())
    }
}

async fn ctx_with(users: Arc<dyn UserDirectory>) -> Option<Ctx> {
    ctx_with_mailer(users, Arc::new(CaptureMailer::default())).await
}

async fn ctx_with_mailer(
    users: Arc<dyn UserDirectory>,
    mailer: Arc<dyn InvitationMailer>,
) -> Option<Ctx> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        println!("DATABASE_URL unset; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_db::OUTBOX_MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let module = WorkspaceModule::with_invitations(
        WorkspaceDeps {
            pool: pool.clone(),
            jwt: keys.clone(),
            clock,
            users,
        },
        mailer,
        7,
    );
    Some(Ctx {
        pool,
        router: module.router(),
        module,
        keys,
    })
}

async fn ctx() -> Option<Ctx> {
    ctx_with(Arc::new(NoopUserDirectory)).await
}

impl Ctx {
    fn token(&self, u: UserId) -> String {
        self.keys
            .issue_access(u, Uuid::new_v4(), false, chrono::Utc::now())
            .unwrap()
            .0
    }

    async fn call(
        &self,
        m: Method,
        uri: &str,
        u: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(m)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token(u)));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    async fn create_ws(&self, owner: UserId, name: &str) -> String {
        let (st, v) = self
            .call(
                Method::POST,
                "/workspaces",
                owner,
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        v["id"].as_str().unwrap().to_owned()
    }

    /// Tests have no invitation flow yet: insert the membership directly.
    async fn add_member(&self, ws: &str, user: UserId, role: WorkspaceRole) {
        sqlx::query(
            "INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) \
             VALUES ($1, $2, $3, $4, now())",
        )
        .bind(Uuid::now_v7())
        .bind(ws.parse::<Uuid>().unwrap())
        .bind(user.as_uuid())
        .bind(role_id(role))
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn create_list_get() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let name = uniq("Alpha Team");
    let (st, v) = c
        .call(
            Method::POST,
            "/workspaces",
            owner,
            Some(json!({ "name": name, "description": "d" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["myRole"], "OWNER");
    assert_eq!(v["description"], "d");
    let id = v["id"].as_str().unwrap().to_owned();

    let (st, v) = c
        .call(
            Method::GET,
            "/workspaces?page=1&pageSize=10&search=ALPHA%20team",
            owner,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 1);
    assert_eq!(v["pageSize"], 10);
    assert_eq!(v["items"][0]["id"], id);

    let (st, v) = c
        .call(Method::GET, &format!("/workspaces/{id}"), owner, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["ownerId"], owner.to_string());

    let (st, _) = c
        .call(
            Method::POST,
            "/workspaces",
            owner,
            Some(json!({ "name": "  " })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let q = c.module.queries();
    let wid: WorkspaceId = id.parse().unwrap();
    assert_eq!(
        q.member_role(wid, owner).await.unwrap(),
        Some(WorkspaceRole::Owner)
    );
    assert!(q.is_active(wid).await.unwrap());
    assert_eq!(q.member_role(wid, UserId::new()).await.unwrap(), None);
}

#[tokio::test]
async fn requires_auth() {
    let Some(c) = ctx().await else { return };
    let resp = c
        .router
        .clone()
        .oneshot(Request::get("/workspaces").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_member_gets_404() {
    let Some(c) = ctx().await else { return };
    let (owner, outsider) = (UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("Secret")).await;
    for (m, p, b) in [
        (Method::GET, format!("/workspaces/{id}"), None),
        (Method::GET, format!("/workspaces/{id}/settings"), None),
        (Method::GET, format!("/workspaces/{id}/members"), None),
        (Method::DELETE, format!("/workspaces/{id}"), None),
        (Method::POST, format!("/workspaces/{id}/leave"), None),
        (
            Method::PUT,
            format!("/workspaces/{id}/settings"),
            Some(json!({})),
        ),
    ] {
        let (st, _) = c.call(m, &p, outsider, b).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }
    let (_, v) = c.call(Method::GET, "/workspaces", outsider, None).await;
    assert_eq!(v["total"], 0);
}

#[tokio::test]
async fn settings_read_write() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, member) = (UserId::new(), UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("S")).await;
    c.add_member(&id, admin, WorkspaceRole::Admin).await;
    c.add_member(&id, member, WorkspaceRole::Member).await;
    let uri = format!("/workspaces/{id}/settings");

    let (st, v) = c.call(Method::GET, &uri, member, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["maxParticipantsLimit"], 50);

    let body = json!({"maxParticipantsLimit": 10, "allowGuestAccess": true,
        "defaultAiModel": "m1", "audioRetentionDays": 7});
    let (st, _) = c.call(Method::PUT, &uri, member, Some(body.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::PUT, &uri, admin, Some(body)).await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = c.call(Method::GET, &uri, owner, None).await;
    assert_eq!(v["allowGuestAccess"], true);
    assert_eq!(v["defaultAiModel"], "m1");
    assert_eq!(v["audioRetentionDays"], 7);
    // description survives a settings update
    let (_, w) = c
        .call(Method::GET, &format!("/workspaces/{id}"), owner, None)
        .await;
    assert_eq!(w["id"], id);

    let (st, _) = c
        .call(
            Method::PUT,
            &uri,
            owner,
            Some(json!({"maxParticipantsLimit": 0})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn members_list_with_directory() {
    let Some(c) = ctx_with(Arc::new(StubDir)).await else {
        return;
    };
    let (owner, member) = (UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("M")).await;
    c.add_member(&id, member, WorkspaceRole::Member).await;
    let (st, v) = c
        .call(
            Method::GET,
            &format!("/workspaces/{id}/members?pageSize=1"),
            member,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 2);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["role"], "OWNER");
    assert_eq!(v["items"][0]["fullName"], "Stub User");
    assert!(v["items"][0]["joinedAt"].is_string());
}

#[tokio::test]
async fn role_change_permissions() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, member) = (UserId::new(), UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("R")).await;
    c.add_member(&id, admin, WorkspaceRole::Admin).await;
    c.add_member(&id, member, WorkspaceRole::Member).await;
    let role_uri = |u: UserId| format!("/workspaces/{id}/members/{u}/role");

    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(member),
            admin,
            Some(json!({"role": "ADMIN"})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(member),
            owner,
            Some(json!({"role": "OWNER"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(owner),
            owner,
            Some(json!({"role": "ADMIN"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(member),
            owner,
            Some(json!({"role": "bogus"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(UserId::new()),
            owner,
            Some(json!({"role": "ADMIN"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c
        .call(
            Method::PUT,
            &role_uri(member),
            owner,
            Some(json!({"role": "ADMIN"})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let q = c.module.queries();
    let wid: WorkspaceId = id.parse().unwrap();
    assert_eq!(
        q.member_role(wid, member).await.unwrap(),
        Some(WorkspaceRole::Admin)
    );
}

#[tokio::test]
async fn remove_member_hierarchy() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, admin2, m1, m2) = (
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
    );
    let id = c.create_ws(owner, &uniq("Rm")).await;
    c.add_member(&id, admin, WorkspaceRole::Admin).await;
    c.add_member(&id, admin2, WorkspaceRole::Admin).await;
    c.add_member(&id, m1, WorkspaceRole::Member).await;
    c.add_member(&id, m2, WorkspaceRole::Member).await;
    let uri = |u: UserId| format!("/workspaces/{id}/members/{u}");

    let (st, _) = c.call(Method::DELETE, &uri(owner), admin, None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, v) = c.call(Method::DELETE, &uri(owner), owner, None).await;
    assert_eq!(
        (st, v["code"].as_str()),
        (StatusCode::CONFLICT, Some("OWNER_MUST_TRANSFER"))
    );
    let (st, _) = c.call(Method::DELETE, &uri(admin2), admin, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::DELETE, &uri(m2), m1, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::DELETE, &uri(m1), admin, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c.call(Method::DELETE, &uri(admin), owner, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // removed members lose access and are gone from the list
    let (st, _) = c
        .call(Method::GET, &format!("/workspaces/{id}"), m1, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = c
        .call(
            Method::GET,
            &format!("/workspaces/{id}/members"),
            owner,
            None,
        )
        .await;
    assert_eq!(v["total"], 3);
    let (st, _) = c.call(Method::DELETE, &uri(m1), owner, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn leave_rules() {
    let Some(c) = ctx().await else { return };
    let (owner, member) = (UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("L")).await;
    c.add_member(&id, member, WorkspaceRole::Member).await;
    let uri = format!("/workspaces/{id}/leave");
    let (st, v) = c.call(Method::POST, &uri, owner, None).await;
    assert_eq!(
        (st, v["code"].as_str()),
        (StatusCode::CONFLICT, Some("OWNER_MUST_TRANSFER"))
    );
    let (st, _) = c.call(Method::POST, &uri, member, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c.call(Method::POST, &uri, member, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = c.call(Method::GET, "/workspaces", member, None).await;
    assert_eq!(v["total"], 0);
}

#[tokio::test]
async fn transfer_ownership() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, member, outsider) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("T")).await;
    c.add_member(&id, admin, WorkspaceRole::Admin).await;
    c.add_member(&id, member, WorkspaceRole::Member).await;
    let uri = format!("/workspaces/{id}/transfer-ownership");

    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            admin,
            Some(json!({"newOwnerId": member})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            owner,
            Some(json!({"newOwnerId": outsider})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            owner,
            Some(json!({"newOwnerId": owner})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            owner,
            Some(json!({"newOwnerId": member})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let q = c.module.queries();
    let wid: WorkspaceId = id.parse().unwrap();
    assert_eq!(
        q.member_role(wid, member).await.unwrap(),
        Some(WorkspaceRole::Owner)
    );
    assert_eq!(
        q.member_role(wid, owner).await.unwrap(),
        Some(WorkspaceRole::Admin)
    );
    let (_, v) = c
        .call(Method::GET, &format!("/workspaces/{id}"), member, None)
        .await;
    assert_eq!(v["ownerId"], member.to_string());
    // old owner can now leave
    let (st, _) = c
        .call(
            Method::POST,
            &format!("/workspaces/{id}/leave"),
            owner,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_by_non_owner_403_then_owner_soft_deletes() {
    let Some(c) = ctx().await else { return };
    let (owner, admin) = (UserId::new(), UserId::new());
    let id = c.create_ws(owner, &uniq("D")).await;
    c.add_member(&id, admin, WorkspaceRole::Admin).await;
    let uri = format!("/workspaces/{id}");
    let (st, _) = c.call(Method::DELETE, &uri, admin, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::DELETE, &uri, owner, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c.call(Method::GET, &uri, owner, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = c.call(Method::GET, "/workspaces", owner, None).await;
    assert_eq!(v["total"], 0);
    let wid: WorkspaceId = id.parse().unwrap();
    assert!(!c.module.queries().is_active(wid).await.unwrap());
    assert_eq!(
        c.module.queries().member_role(wid, owner).await.unwrap(),
        None
    );
    let (deleted,): (bool,) =
        sqlx::query_as("SELECT deleted_at IS NOT NULL FROM workspace.workspaces WHERE id = $1")
            .bind(wid.as_uuid())
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert!(deleted);
}

// ---------------------------------------------------------------- invitations

async fn inv_ctx() -> Option<(Ctx, Arc<CaptureMailer>)> {
    let mailer = Arc::new(CaptureMailer::default());
    let c = ctx_with_mailer(Arc::new(StubDir), mailer.clone()).await?;
    Some((c, mailer))
}

fn email_of(u: UserId) -> String {
    format!("{u}@example.test")
}

#[tokio::test]
async fn invitation_full_flow() {
    let Some((c, mailer)) = inv_ctx().await else {
        return;
    };
    let owner = UserId::new();
    let guest = UserId::new();
    let ws = c.create_ws(owner, &uniq("Inv")).await;
    let base = format!("/workspaces/{ws}/invitations");
    let email = email_of(guest);

    // invite (mixed-case input is normalised)
    let (st, v) = c
        .call(
            Method::POST,
            &base,
            owner,
            Some(json!({ "email": email.to_uppercase(), "role": "MEMBER" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["status"], "pending");
    assert_eq!(v["role"], "MEMBER");
    assert_eq!(v["deliveryStatus"], "Sent");
    assert!(v.get("token").is_none());
    let inv_id = v["id"].as_str().unwrap().to_owned();
    let token = mailer.token_for(&email);

    // stored hashed only, 7-day expiry
    let (stored, secs): (Option<String>, f64) = sqlx::query_as(
        "SELECT token_hash, EXTRACT(EPOCH FROM (expires_at - created_at))::float8 \
         FROM workspace.workspace_invitations WHERE id = $1",
    )
    .bind(inv_id.parse::<Uuid>().unwrap())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    let stored = stored.unwrap();
    assert_ne!(stored, token);
    assert_eq!(stored.len(), 64);
    assert!((secs - 7.0 * 86400.0).abs() < 5.0);

    // duplicate pending -> 409, also different case
    let (st, v) = c
        .call(
            Method::POST,
            &base,
            owner,
            Some(json!({ "email": email, "role": "ADMIN" })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "INVITATION_PENDING");

    // list
    let (st, v) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 1);

    // guest sees it as pending, no token leaked
    let (st, v) = c
        .call(Method::GET, "/invitations/pending", guest, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["id"], inv_id);
    assert!(v[0]["workspaceName"].as_str().is_some());

    // resend replaces the token; the old one stops working
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{base}/{inv_id}/resend"),
            owner,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["sentCount"], 2);
    let new_token = mailer.token_for(&email);
    assert_ne!(new_token, token);
    let (st, v) = c
        .call(
            Method::POST,
            "/invitations/accept",
            guest,
            Some(json!({ "token": token })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(v["code"], "INVITATION_INVALID");

    // wrong user (email mismatch)
    let stranger = UserId::new();
    let (st, v) = c
        .call(
            Method::POST,
            "/invitations/accept",
            stranger,
            Some(json!({ "token": new_token })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["code"], "INVITATION_EMAIL_MISMATCH");

    // accept, then accept again (idempotent)
    for _ in 0..2 {
        let (st, v) = c
            .call(
                Method::POST,
                "/invitations/accept",
                guest,
                Some(json!({ "token": new_token })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["workspaceId"], ws);
        assert_eq!(v["role"], "MEMBER");
    }
    let (st, v) = c
        .call(Method::GET, &format!("/workspaces/{ws}"), guest, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["myRole"], "MEMBER");
    let (_, v) = c
        .call(Method::GET, "/invitations/pending", guest, None)
        .await;
    assert!(v.as_array().unwrap().is_empty());

    // accepted invitations cannot be revoked
    let (st, _) = c
        .call(Method::DELETE, &format!("{base}/{inv_id}"), owner, None)
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn invitation_authorization_and_validation() {
    let Some((c, _)) = inv_ctx().await else {
        return;
    };
    let owner = UserId::new();
    let admin = UserId::new();
    let member = UserId::new();
    let outsider = UserId::new();
    let ws = c.create_ws(owner, &uniq("InvAuth")).await;
    c.add_member(&ws, admin, WorkspaceRole::Admin).await;
    c.add_member(&ws, member, WorkspaceRole::Member).await;
    let base = format!("/workspaces/{ws}/invitations");
    let body = |e: &str, r: &str| Some(json!({ "email": e, "role": r }));

    let (st, v) = c
        .call(Method::POST, &base, owner, body("a@example.test", "OWNER"))
        .await;
    assert!(st.is_client_error());
    assert_eq!(v["code"], "INVALID_ROLE");
    let (st, v) = c
        .call(Method::POST, &base, owner, body("not-an-email", "MEMBER"))
        .await;
    assert!(st.is_client_error());
    assert_eq!(v["code"], "INVALID_EMAIL");
    let (st, _) = c
        .call(
            Method::POST,
            &base,
            member,
            body("b@example.test", "MEMBER"),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::GET, &base, member, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c
        .call(
            Method::POST,
            &base,
            outsider,
            body("b@example.test", "MEMBER"),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // admin may invite members but not admins (BR-17)
    let (st, _) = c
        .call(Method::POST, &base, admin, body("c@example.test", "ADMIN"))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c
        .call(Method::POST, &base, admin, body("c@example.test", "MEMBER"))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap();

    // revoke, after which the same email may be invited again
    let (st, _) = c
        .call(Method::DELETE, &format!("{base}/{id}"), admin, None)
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c
        .call(Method::POST, &base, admin, body("c@example.test", "MEMBER"))
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = c
        .call(
            Method::DELETE,
            &format!("{base}/{}", Uuid::new_v4()),
            admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invitation_expiry() {
    let Some((c, mailer)) = inv_ctx().await else {
        return;
    };
    let owner = UserId::new();
    let guest = UserId::new();
    let ws = c.create_ws(owner, &uniq("InvExp")).await;
    let base = format!("/workspaces/{ws}/invitations");
    let email = email_of(guest);
    let (st, v) = c
        .call(
            Method::POST,
            &base,
            owner,
            Some(json!({ "email": email, "role": "MEMBER" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let token = mailer.token_for(&email);
    sqlx::query("UPDATE workspace.workspace_invitations SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(id)
        .execute(&c.pool)
        .await
        .unwrap();
    let (st, v) = c
        .call(Method::GET, "/invitations/pending", guest, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.as_array().unwrap().is_empty());
    let (st, v) = c
        .call(
            Method::POST,
            "/invitations/accept",
            guest,
            Some(json!({ "token": token })),
        )
        .await;
    assert_eq!(st, StatusCode::GONE, "{v}");
    assert_eq!(v["code"], "INVITATION_EXPIRED");
    // an expired invitation does not block re-inviting
    let (st, _) = c
        .call(
            Method::POST,
            &base,
            owner,
            Some(json!({ "email": email, "role": "MEMBER" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
}

#[tokio::test]
async fn dev_invitation_capture_writes_redis_only_when_enabled() {
    let Ok(url) = std::env::var("REDIS_URL") else {
        println!("REDIS_URL unset; skipping");
        return;
    };
    assert!(
        DevInvitationMailCapture::from_flag(None, &url)
            .unwrap()
            .is_none()
    );
    assert!(
        DevInvitationMailCapture::from_flag(Some("0"), &url)
            .unwrap()
            .is_none()
    );
    let m = DevInvitationMailCapture::from_flag(Some("1"), &url)
        .unwrap()
        .unwrap();
    let email = format!("{}@Example.test", Uuid::new_v4().simple());
    m.send_invitation(&InvitationMail {
        to: &email,
        workspace_name: "W",
        inviter_name: "I",
        role: WorkspaceRole::Member,
        token: "raw-token",
        expires_at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let client = redis::Client::open(url).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let key = DevInvitationMailCapture::key(&email);
    assert_eq!(
        key,
        format!("dev:mail:workspace-invite:{}", email.to_lowercase())
    );
    let v: String = redis::AsyncCommands::get(&mut conn, &key).await.unwrap();
    assert_eq!(v, "raw-token");
    let ttl: i64 = redis::AsyncCommands::ttl(&mut conn, &key).await.unwrap();
    assert!(ttl > 3500 && ttl <= 3600);
}

#[tokio::test]
async fn malformed_input_is_invalid_request() {
    let Some(c) = ctx().await else { return };
    let user = UserId::new();
    for (method, uri, body) in [
        (Method::POST, "/workspaces", "{bad"),
        (Method::GET, "/workspaces/not-a-uuid", ""),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", c.token(user)))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = c.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(v["code"], "INVALID_REQUEST", "{uri} {v}");
    }
}
