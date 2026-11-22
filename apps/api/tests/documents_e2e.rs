//! Workspace documents through the full API router: multipart upload, list by role, the
//! index request on Redis, and the knowledge allowlist adapter.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_api::wiring::KnowledgeAccessAdapter;
use warptalk_bus::{StreamHandler, StreamMessage};
use warptalk_config::JwtConfig;
use warptalk_identity::VerificationMailer;
use warptalk_kernel::{AppError, AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::DocumentAccess;
use warptalk_storage::LocalFsStore;
use warptalk_web::JwtKeys;
use warptalk_workspace::{
    DocumentsConfig, InvitationMail, InvitationMailer, NoopUserDirectory, WorkspaceDeps,
    WorkspaceModule,
};

#[derive(Default)]
struct CaptureMailer(Mutex<Vec<(String, String)>>);

#[async_trait::async_trait]
impl VerificationMailer for CaptureMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push((email.to_owned(), token.to_owned()));
        Ok(())
    }
}

struct NoInvites;

#[async_trait::async_trait]
impl InvitationMailer for NoInvites {
    async fn send_invitation(&self, _mail: &InvitationMail<'_>) -> AppResult<()> {
        Ok(())
    }
}

async fn call_raw(
    app: &Router,
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    bearer: &str,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"));
    if let Some(ct) = content_type {
        req = req.header("content-type", ct);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: &str,
) -> (StatusCode, Value) {
    match body {
        Some(b) => {
            call_raw(
                app,
                method,
                uri,
                Some("application/json"),
                b.to_string().into_bytes(),
                bearer,
            )
            .await
        }
        None => call_raw(app, method, uri, None, Vec::new(), bearer).await,
    }
}

/// Registers, verifies and logs in a fresh user; returns `(user_id, access_token)`.
async fn new_user(app: &Router, mailer: &CaptureMailer, name: &str) -> (UserId, String) {
    let email = format!("docs-{}@example.test", Uuid::new_v4().simple());
    let pw = "S3cret-pass!";
    let anon = |m: &'static str, uri: &'static str, b: Value| {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(m)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(Body::from(b.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let st = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                st,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let (s, b) = anon(
        "POST",
        "/api/v1/auth/register",
        json!({"email": email, "password": pw, "fullName": name}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let user_id: UserId = b["userId"].as_str().unwrap().parse().unwrap();
    let token = mailer
        .0
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(e, _)| *e == email)
        .map(|(_, t)| t.clone())
        .unwrap();
    let (s, _) = anon("POST", "/api/v1/auth/verify-email", json!({"token": token})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, b) = anon(
        "POST",
        "/api/v1/auth/login",
        json!({"email": email, "password": pw}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    (user_id, b["accessToken"].as_str().unwrap().to_owned())
}

fn multipart(file_name: &str, data: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = format!("----wt{}", Uuid::new_v4().simple());
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn documents_upload_list_and_knowledge_allowlist() {
    let (Ok(url), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        eprintln!("DATABASE_URL / REDIS_URL not set; skipping documents e2e");
        return;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_db::OUTBOX_MIGRATIONS,
            warptalk_settings::MIGRATIONS,
            warptalk_identity::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
            warptalk_room::MIGRATIONS,
            warptalk_meeting::MIGRATIONS,
            warptalk_transcript::MIGRATIONS,
            warptalk_knowledge::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let cfg = JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    };
    let keys = JwtKeys::from_config(&cfg);
    let mailer = Arc::new(CaptureMailer::default());
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let store = Arc::new(LocalFsStore::new(
        std::env::temp_dir().join(format!("wt-api-docs-{}", Uuid::new_v4())),
    ));
    let app = warptalk_api::build_router(
        pool.clone(),
        keys.clone(),
        &cfg,
        &[],
        mailer.clone(),
        Some(redis.clone()),
        warptalk_api::ApiOptions {
            invitation_mailer: Some(Arc::new(NoInvites)),
            object_store: Some(store.clone()),
            documents_config: Some(DocumentsConfig::default()),
            ..warptalk_api::ApiOptions::default()
        },
    )
    .await;

    let (_, owner) = new_user(&app, &mailer, "Docs Owner").await;
    let (member_id, member) = new_user(&app, &mailer, "Docs Member").await;
    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": format!("Docs {}", Uuid::new_v4().simple())})),
        &owner,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let ws = b["id"].as_str().unwrap().to_owned();
    let ws_id: WorkspaceId = ws.parse().unwrap();
    sqlx::query(
        "INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) \
         VALUES ($1, $2, $3, $4, now())",
    )
    .bind(Uuid::now_v7())
    .bind(ws_id.as_uuid())
    .bind(member_id.as_uuid())
    .bind(warptalk_workspace::domain::role_id(WorkspaceRole::Member))
    .execute(&pool)
    .await
    .unwrap();

    // Upload as owner (multipart through /api/v1).
    let mut ids = Vec::new();
    for (name, level) in [("handbook.md", "MEMBERS_ONLY"), ("board.txt", "ADMIN_ONLY")] {
        let (ct, body) = multipart(
            name,
            b"# Title\nBody text for the assistant.",
            &[
                ("title", name),
                ("category", "GENERAL"),
                ("accessLevel", level),
                ("aiRetrievalAllowed", "true"),
            ],
        );
        let (s, b) = call_raw(
            &app,
            "POST",
            &format!("/api/v1/workspaces/{ws}/documents"),
            Some(&ct),
            body,
            &owner,
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        assert_eq!(b["ingestionStatus"], "pending");
        ids.push(b["id"].as_str().unwrap().to_owned());
    }
    // Members cannot upload.
    let (ct, body) = multipart("x.txt", b"x", &[]);
    let (s, b) = call_raw(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(&ct),
        body,
        &member,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");

    // The index request is on the stream.
    let mut conn = redis.clone();
    let entries: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
        .arg("embedding:index_requests")
        .arg("-")
        .arg("+")
        .query_async(&mut conn)
        .await
        .unwrap();
    let found = entries.ids.iter().any(|sid| {
        sid.map
            .get("data")
            .and_then(|v| redis::from_redis_value::<String>(v).ok())
            .is_some_and(|d| d.contains(&ids[0]) && d.contains(&format!("workspace_{ws}")))
    });
    assert!(found, "index request for {} not on the stream", ids[0]);

    // List by role.
    let (s, b) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents"),
        None,
        &owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["total"], 2);
    let (s, b) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents"),
        None,
        &member,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["total"], 1);
    assert_eq!(b["items"][0]["id"], ids[0].as_str());
    assert_eq!(b["items"][0]["accessLevel"], "MEMBERS_ONLY");

    // Knowledge allowlist: empty until indexed, then the member's visible document only.
    let workspace = WorkspaceModule::new(WorkspaceDeps {
        pool: pool.clone(),
        jwt: keys,
        clock: Arc::new(SystemClock),
        users: Arc::new(NoopUserDirectory),
    });
    let adapter = KnowledgeAccessAdapter(workspace.queries());
    let scope = adapter.access_scope(ws_id, member_id).await.unwrap();
    assert_eq!(scope.role, WorkspaceRole::Member);
    assert!(scope.allowed_document_ids.is_empty());
    let handler = workspace.document_results_handler();
    for id in &ids {
        handler
            .handle(&StreamMessage {
                stream: "embedding:index_results".into(),
                id: "0-1".into(),
                fields: [
                    ("job_id", format!("{id}:1")),
                    ("workspace_id", ws.clone()),
                    ("source_type", "document".into()),
                    ("source_id", id.clone()),
                    ("status", "indexed".into()),
                    ("chunks_indexed", "2".into()),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            })
            .await
            .unwrap();
    }
    let scope = adapter.access_scope(ws_id, member_id).await.unwrap();
    assert_eq!(
        scope.allowed_document_ids,
        vec![ids[0].parse::<Uuid>().unwrap()]
    );

    // Archive removes it from the allowlist and the list.
    let (s, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/documents/{}", ids[0]),
        None,
        &owner,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let scope = adapter.access_scope(ws_id, member_id).await.unwrap();
    assert!(scope.allowed_document_ids.is_empty());
    let (_, b) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents"),
        None,
        &member,
    )
    .await;
    assert_eq!(b["total"], 0);

    // Non-members are rejected.
    let (_, stranger_token) = new_user(&app, &mailer, "Stranger").await;
    let stranger = {
        // access scope for a stranger is Forbidden
        let (s, _) = call(
            &app,
            "GET",
            &format!("/api/v1/workspaces/{ws}/documents"),
            None,
            &stranger_token,
        )
        .await;
        s
    };
    assert_eq!(stranger, StatusCode::NOT_FOUND);
}
