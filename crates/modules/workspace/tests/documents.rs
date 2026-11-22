#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use redis::aio::ConnectionManager;
use redis::streams::StreamRangeReply;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_bus::{StreamHandler, StreamMessage};
use warptalk_config::JwtConfig;
use warptalk_contracts::streams::EMBEDDING_INDEX_REQUESTS;
use warptalk_kernel::{Clock, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_storage::{LocalFsStore, ObjectStore};
use warptalk_web::JwtKeys;
use warptalk_workspace::domain::role_id;
use warptalk_workspace::{
    DocumentsConfig, DocumentsDeps, MIGRATIONS, NoopUserDirectory, RedisDocumentIndexPublisher,
    WorkspaceDeps, WorkspaceModule,
};

const PURGE_STREAM: &str = "knowledge:purge_requests";

struct Ctx {
    pool: PgPool,
    router: Router,
    module: WorkspaceModule,
    keys: JwtKeys,
    store: Arc<LocalFsStore>,
    redis: ConnectionManager,
}

async fn ctx_with(max_upload_bytes: u64) -> Option<Ctx> {
    let (Ok(url), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
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
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let store = Arc::new(LocalFsStore::new(
        std::env::temp_dir().join(format!("wt-docs-{}", Uuid::new_v4())),
    ));
    let module = WorkspaceModule::new(WorkspaceDeps {
        pool: pool.clone(),
        jwt: keys.clone(),
        clock,
        users: Arc::new(NoopUserDirectory),
    })
    .with_documents(DocumentsDeps {
        store: store.clone(),
        publisher: Arc::new(RedisDocumentIndexPublisher::new(redis.clone())),
        config: DocumentsConfig {
            max_upload_bytes,
            ..DocumentsConfig::default()
        },
    });
    Some(Ctx {
        pool,
        router: module.router(),
        module,
        keys,
        store,
        redis,
    })
}

async fn ctx() -> Option<Ctx> {
    ctx_with(20 * 1024 * 1024).await
}

struct Part<'a> {
    name: &'a str,
    file_name: Option<&'a str>,
    data: &'a [u8],
}

fn text(name: &'static str, v: &str) -> Part<'static> {
    // Leak-free: the value is copied into the body immediately, so borrow through a Box::leak
    // keeps the helper signature simple in tests.
    Part {
        name,
        file_name: None,
        data: Box::leak(v.to_owned().into_boxed_str()).as_bytes(),
    }
}

fn multipart(parts: &[Part<'_>]) -> (String, Vec<u8>) {
    let boundary = format!("----wt{}", Uuid::new_v4().simple());
    let mut body = Vec::new();
    for p in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match p.file_name {
            Some(f) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{f}\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n",
                    p.name
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                    p.name
                )
                .as_bytes(),
            ),
        }
        body.extend_from_slice(p.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl Ctx {
    fn token(&self, u: UserId) -> String {
        self.keys
            .issue_access(u, Uuid::new_v4(), false, chrono::Utc::now())
            .unwrap()
            .0
    }

    async fn raw(
        &self,
        m: Method,
        uri: &str,
        u: UserId,
        content_type: Option<String>,
        body: Vec<u8>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut b = Request::builder()
            .method(m)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token(u)));
        if let Some(ct) = content_type {
            b = b.header("content-type", ct);
        }
        let resp = self
            .router
            .clone()
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, bytes)
    }

    async fn call(
        &self,
        m: Method,
        uri: &str,
        u: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (ct, bytes) = match body {
            Some(v) => (
                Some("application/json".to_owned()),
                v.to_string().into_bytes(),
            ),
            None => (None, Vec::new()),
        };
        let (st, _, out) = self.raw(m, uri, u, ct, bytes).await;
        (st, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }

    async fn upload(
        &self,
        ws: &str,
        u: UserId,
        file_name: &str,
        data: &[u8],
        extra: &[Part<'_>],
    ) -> (StatusCode, Value) {
        let mut parts = vec![Part {
            name: "file",
            file_name: Some(file_name),
            data,
        }];
        parts.extend(extra.iter().map(|p| Part {
            name: p.name,
            file_name: p.file_name,
            data: p.data,
        }));
        let (ct, body) = multipart(&parts);
        let (st, _, out) = self
            .raw(
                Method::POST,
                &format!("/workspaces/{ws}/documents"),
                u,
                Some(ct),
                body,
            )
            .await;
        (st, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }

    async fn create_ws(&self, owner: UserId) -> String {
        let (st, v) = self
            .call(
                Method::POST,
                "/workspaces",
                owner,
                Some(json!({ "name": format!("Docs {}", Uuid::new_v4().simple()) })),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        v["id"].as_str().unwrap().to_owned()
    }

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

    /// Entries of `stream` whose `data.source_id` is `doc`, oldest first.
    async fn stream_entries(&self, stream: &str, doc: &str) -> Vec<Value> {
        let mut conn = self.redis.clone();
        let reply: StreamRangeReply = redis::cmd("XRANGE")
            .arg(stream)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        reply
            .ids
            .iter()
            .filter_map(|sid| {
                let raw = sid.map.get("data")?;
                let s: String = redis::from_redis_value(raw).ok()?;
                let v: Value = serde_json::from_str(&s).ok()?;
                (v["source_id"] == doc).then_some(v)
            })
            .collect()
    }

    async fn result(&self, fields: &[(&str, String)]) {
        let msg = StreamMessage {
            stream: "embedding:index_results".into(),
            id: "0-1".into(),
            fields: fields
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        };
        self.module
            .document_results_handler()
            .handle(&msg)
            .await
            .unwrap();
    }
}

fn result_fields(
    ws: &str,
    doc: &str,
    rev: i32,
    status: &str,
    chunks: u32,
) -> Vec<(&'static str, String)> {
    vec![
        ("job_id", format!("{doc}:{rev}")),
        ("workspace_id", ws.to_owned()),
        ("source_type", "document".to_owned()),
        ("source_id", doc.to_owned()),
        ("status", status.to_owned()),
        ("chunks_indexed", chunks.to_string()),
        ("provider", "p".to_owned()),
        ("model", "m".to_owned()),
        ("dimensions", "8".to_owned()),
        ("reason", String::new()),
    ]
}

#[tokio::test]
async fn upload_txt_stores_object_row_and_index_request() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let (st, v) = c
        .upload(
            &ws,
            owner,
            "../notes.txt",
            b"Quarterly plan\nShip the thing.",
            &[
                text("title", "Q3 plan"),
                text("category", "technical"),
                text("accessLevel", "MEMBERS_ONLY"),
                text("aiRetrievalAllowed", "true"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["title"], "Q3 plan");
    assert_eq!(v["fileName"], "notes.txt");
    assert_eq!(v["category"], "TECHNICAL");
    assert_eq!(v["accessLevel"], "MEMBERS_ONLY");
    assert_eq!(v["aiRetrievalAllowed"], true);
    assert_eq!(v["aiEligible"], false);
    assert_eq!(v["ingestionStatus"], "pending");
    assert_eq!(v["mimeType"], "text/plain");
    assert_eq!(v["sizeBytes"], 30);
    let id = v["id"].as_str().unwrap();

    // Row.
    let (key, provider): (String, String) = sqlx::query_as(
        "SELECT storage_key, storage_provider FROM workspace.workspace_documents \
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .bind(ws.parse::<Uuid>().unwrap())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(provider, "local");
    // Object.
    assert_eq!(
        &c.store.get(&key).await.unwrap()[..],
        b"Quarterly plan\nShip the thing."
    );
    // Index request.
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, id).await;
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    let r = &reqs[0];
    assert_eq!(r["job_id"], format!("{id}:1"));
    assert_eq!(r["workspace_id"], ws);
    assert_eq!(r["collection_id"], format!("workspace_{ws}"));
    assert_eq!(r["source_type"], "document");
    assert_eq!(r["title"], "Q3 plan");
    assert_eq!(r["ai_retrieval_allowed"], true);
    assert_eq!(r["deletion_state"], "active");
    assert_eq!(r["needs_extraction"], false);
    assert_eq!(r["content"]["type"], "document");
    assert_eq!(
        r["content"]["pages"][0]["text"],
        "Quarterly plan\nShip the thing."
    );
}

#[tokio::test]
async fn pdf_is_sent_for_extraction_and_ai_disallowed_upload_is_not_indexed() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let (st, v) = c
        .upload(&ws, owner, "spec.pdf", b"%PDF-1.4\n%binary", &[])
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["title"], "spec");
    assert_eq!(v["category"], "GENERAL");
    assert_eq!(v["accessLevel"], "MEMBERS_ONLY");
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["needs_extraction"], true);
    assert!(reqs[0].get("content").is_none());
    assert!(
        reqs[0]["object_key"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{id}/original.pdf"))
    );
    assert_eq!(reqs[0]["mime_type"], "application/pdf");

    let (st, v) = c
        .upload(
            &ws,
            owner,
            "private.md",
            b"# secret",
            &[text("aiRetrievalAllowed", "false")],
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["ingestionStatus"], "skipped");
    assert_eq!(v["ingestionFailureReason"], "ai_retrieval_not_allowed");
    let id2 = v["id"].as_str().unwrap();
    assert!(
        c.stream_entries(EMBEDDING_INDEX_REQUESTS, id2)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn rbac_on_upload_patch_archive() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, member, outsider) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let ws = c.create_ws(owner).await;
    c.add_member(&ws, admin, WorkspaceRole::Admin).await;
    c.add_member(&ws, member, WorkspaceRole::Member).await;

    let (st, _) = c.upload(&ws, member, "a.txt", b"hello", &[]).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.upload(&ws, outsider, "a.txt", b"hello", &[]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, v) = c.upload(&ws, admin, "a.txt", b"hello", &[]).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap();
    let base = format!("/workspaces/{ws}/documents/{id}");

    let (st, _) = c
        .call(Method::PATCH, &base, member, Some(json!({ "title": "x" })))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::DELETE, &base, member, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::GET, &base, outsider, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c
        .call(
            Method::GET,
            &format!("/workspaces/{ws}/documents"),
            outsider,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c.call(Method::GET, &base, member, None).await;
    assert_eq!(st, StatusCode::OK);

    // A document of another workspace is not reachable through this one.
    let other_ws = c.create_ws(UserId::new()).await;
    let (st, _) = c
        .call(
            Method::GET,
            &format!("/workspaces/{other_ws}/documents/{id}"),
            owner,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let resp = c
        .router
        .clone()
        .oneshot(
            Request::get(format!("/workspaces/{ws}/documents"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_oversize_unsupported_and_mismatched_files() {
    let Some(c) = ctx_with(1024).await else {
        return;
    };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;

    let big = vec![b'a'; 2048];
    let (st, v) = c.upload(&ws, owner, "big.txt", &big, &[]).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{v}");
    assert_eq!(v["code"], "FILE_TOO_LARGE");
    assert_eq!(v["message"], "File exceeds the 1 KB limit.");

    let (st, v) = c.upload(&ws, owner, "run.exe", b"MZ....", &[]).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");
    assert_eq!(v["code"], "UNSUPPORTED_FILE_TYPE");
    let (st, v) = c.upload(&ws, owner, "noext", b"hello", &[]).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");

    // Extension and magic bytes must agree.
    let (st, v) = c.upload(&ws, owner, "fake.pdf", b"just text", &[]).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");
    assert_eq!(v["code"], "FILE_CONTENT_MISMATCH");
    let (st, v) = c.upload(&ws, owner, "fake.docx", b"%PDF-1.4", &[]).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");
    let (st, v) = c
        .upload(&ws, owner, "bin.txt", b"%PDF\x00\x01\x02", &[])
        .await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");

    let (st, v) = c.upload(&ws, owner, "empty.txt", b"", &[]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "EMPTY_FILE");
    let (st, v) = c
        .upload(
            &ws,
            owner,
            "a.txt",
            b"x",
            &[text("accessLevel", "EVERYONE")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "INVALID_ACCESS_LEVEL");
    let (st, v) = c
        .upload(&ws, owner, "a.txt", b"x", &[text("category", "MISC")])
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");

    // No `file` part.
    let (ct, body) = multipart(&[text("title", "t")]);
    let (st, _, out) = c
        .raw(
            Method::POST,
            &format!("/workspaces/{ws}/documents"),
            owner,
            Some(ct),
            body,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&out).contains("FILE_REQUIRED"));

    // Nothing was stored.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace.workspace_documents WHERE workspace_id = $1",
    )
    .bind(ws.parse::<Uuid>().unwrap())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn list_and_get_respect_access_level() {
    let Some(c) = ctx().await else { return };
    let (owner, admin, member) = (UserId::new(), UserId::new(), UserId::new());
    let ws = c.create_ws(owner).await;
    c.add_member(&ws, admin, WorkspaceRole::Admin).await;
    c.add_member(&ws, member, WorkspaceRole::Member).await;

    let mut ids = Vec::new();
    for (name, level) in [
        ("pub.txt", "PUBLIC"),
        ("mem.txt", "MEMBERS_ONLY"),
        ("adm.txt", "ADMIN_ONLY"),
    ] {
        let (st, v) = c
            .upload(&ws, owner, name, b"content", &[text("accessLevel", level)])
            .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        ids.push(v["id"].as_str().unwrap().to_owned());
    }
    let list = |u: UserId, q: &str| {
        let uri = format!("/workspaces/{ws}/documents{q}");
        let c = &c;
        async move { c.call(Method::GET, &uri, u, None).await }
    };
    let (st, v) = list(member, "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 2);
    let names: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["fileName"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"adm.txt"), "{names:?}");
    for who in [owner, admin] {
        let (_, v) = list(who, "").await;
        assert_eq!(v["total"], 3);
    }
    let (_, v) = list(owner, "?search=ADM").await;
    assert_eq!(v["total"], 1);
    let (_, v) = list(owner, "?category=LEGAL").await;
    assert_eq!(v["total"], 0);
    let (st, _) = list(owner, "?category=nope").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, v) = list(owner, "?page=2&pageSize=2").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["total"], 3);

    let base = |i: usize| format!("/workspaces/{ws}/documents/{}", ids[i]);
    let (st, _) = c.call(Method::GET, &base(2), member, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = c
        .raw(
            Method::GET,
            &format!("{}/download", base(2)),
            member,
            None,
            vec![],
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c.call(Method::GET, &base(2), admin, None).await;
    assert_eq!(st, StatusCode::OK);

    // Assistant allowlist: nothing until indexed; then only what the role may see.
    let wid: WorkspaceId = ws.parse().unwrap();
    let q = c.module.queries();
    assert!(
        q.ai_retrievable_document_ids(wid, WorkspaceRole::Member)
            .await
            .unwrap()
            .is_empty()
    );
    for id in &ids {
        c.result(&result_fields(&ws, id, 1, "indexed", 2)).await;
    }
    let m = q
        .ai_retrievable_document_ids(wid, WorkspaceRole::Member)
        .await
        .unwrap();
    assert_eq!(m.len(), 2);
    assert!(!m.contains(&ids[2].parse().unwrap()));
    let a = q
        .ai_retrievable_document_ids(wid, WorkspaceRole::Admin)
        .await
        .unwrap();
    assert_eq!(a.len(), 3);
}

#[tokio::test]
async fn download_streams_the_file() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let (_, v) = c
        .upload(&ws, owner, "Résumé.md", "# héllo".as_bytes(), &[])
        .await;
    let id = v["id"].as_str().unwrap();
    let uri = format!("/workspaces/{ws}/documents/{id}/download");
    for q in ["", "?presign=true"] {
        // The local store cannot presign: both modes stream.
        let (st, h, body) = c
            .raw(Method::GET, &format!("{uri}{q}"), owner, None, vec![])
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body, "# héllo".as_bytes());
        assert_eq!(h["content-type"], "text/markdown");
        assert_eq!(h["x-content-type-options"], "nosniff");
        let cd = h["content-disposition"].to_str().unwrap();
        assert!(cd.starts_with("attachment; filename=\""), "{cd}");
        assert!(cd.contains("R%C3%A9sum%C3%A9.md"), "{cd}");
    }
}

#[tokio::test]
async fn patch_publishes_purge_and_reindex() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let (_, v) = c
        .upload(
            &ws,
            owner,
            "kb.txt",
            b"knowledge text",
            &[text("title", "KB")],
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    let base = format!("/workspaces/{ws}/documents/{id}");
    c.result(&result_fields(&ws, &id, 1, "indexed", 1)).await;
    let (_, v) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(v["ingestionStatus"], "completed");
    assert_eq!(v["aiEligible"], true);
    assert!(v["lastIndexedAt"].is_string());

    // Metadata-only change: no index traffic.
    let (st, v) = c
        .call(
            Method::PATCH,
            &base,
            owner,
            Some(json!({ "category": "legal", "accessLevel": "ADMIN_ONLY" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["category"], "LEGAL");
    assert_eq!(v["accessLevel"], "ADMIN_ONLY");
    assert_eq!(v["ingestionStatus"], "completed");
    assert_eq!(
        c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await.len(),
        1
    );
    assert!(c.stream_entries(PURGE_STREAM, &id).await.is_empty());

    // AI off: purge request, blocked index request, not eligible.
    let (st, v) = c
        .call(
            Method::PATCH,
            &base,
            owner,
            Some(json!({ "aiRetrievalAllowed": false })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["aiRetrievalAllowed"], false);
    assert_eq!(v["aiEligible"], false);
    assert_eq!(v["ingestionStatus"], "skipped");
    let purges = c.stream_entries(PURGE_STREAM, &id).await;
    assert_eq!(purges.len(), 1, "{purges:?}");
    assert_eq!(purges[0]["workspace_id"], ws);
    assert_eq!(purges[0]["source_type"], "document");
    assert_eq!(purges[0]["reason"], "ai_retrieval_disallowed");
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await;
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1]["ai_retrieval_allowed"], false);
    // A late "indexed" result must not resurrect it.
    c.result(&result_fields(&ws, &id, 1, "indexed", 1)).await;
    let (_, v) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(v["aiEligible"], false);
    assert_eq!(v["ingestionStatus"], "skipped");

    // AI back on: re-index at revision 2 with the stored text.
    let (_, v) = c
        .call(
            Method::PATCH,
            &base,
            owner,
            Some(json!({ "aiRetrievalAllowed": true })),
        )
        .await;
    assert_eq!(v["ingestionStatus"], "pending");
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await;
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[2]["job_id"], format!("{id}:2"));
    assert_eq!(reqs[2]["content"]["pages"][0]["text"], "knowledge text");

    // Title change re-indexes (citations carry the title).
    let (_, v) = c
        .call(
            Method::PATCH,
            &base,
            owner,
            Some(json!({ "title": "KB v2" })),
        )
        .await;
    assert_eq!(v["title"], "KB v2");
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await;
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[3]["title"], "KB v2");
    assert_eq!(reqs[3]["job_id"], format!("{id}:3"));

    // Stale result of revision 2 is ignored; the current one applies.
    c.result(&result_fields(&ws, &id, 2, "failed", 0)).await;
    let (_, v) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(v["ingestionStatus"], "pending");
    c.result(&result_fields(&ws, &id, 3, "indexed", 5)).await;
    let (_, v) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(v["ingestionStatus"], "completed");
    assert_eq!(v["aiEligible"], true);

    let (st, v) = c
        .call(Method::PATCH, &base, owner, Some(json!({ "title": " " })))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, _) = c
        .call(
            Method::PATCH,
            &base,
            owner,
            Some(json!({ "accessLevel": "X" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn archive_soft_deletes_and_purges() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let (_, v) = c.upload(&ws, owner, "old.txt", b"old", &[]).await;
    let id = v["id"].as_str().unwrap().to_owned();
    let base = format!("/workspaces/{ws}/documents/{id}");
    c.result(&result_fields(&ws, &id, 1, "indexed", 1)).await;

    let (st, _) = c.call(Method::DELETE, &base, owner, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c.call(Method::GET, &base, owner, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c.call(Method::DELETE, &base, owner, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c
        .call(Method::PATCH, &base, owner, Some(json!({ "title": "x" })))
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = c
        .call(
            Method::GET,
            &format!("/workspaces/{ws}/documents"),
            owner,
            None,
        )
        .await;
    assert_eq!(v["total"], 0);

    // Soft: the row and the object remain.
    let (status, deleted_by, key, eligible): (String, Option<Uuid>, String, bool) = sqlx::query_as(
        "SELECT status, deleted_by, storage_key, ai_eligible \
         FROM workspace.workspace_documents WHERE id = $1",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(status, "archived");
    assert_eq!(deleted_by, Some(owner.as_uuid()));
    assert!(!eligible);
    assert!(c.store.get(&key).await.is_ok());

    let purges = c.stream_entries(PURGE_STREAM, &id).await;
    assert_eq!(purges.len(), 1);
    assert_eq!(purges[0]["reason"], "archived");
    let reqs = c.stream_entries(EMBEDDING_INDEX_REQUESTS, &id).await;
    assert_eq!(reqs.last().unwrap()["deletion_state"], "deleted");

    // `deleted` / late `indexed` results leave the archived row alone.
    c.result(&result_fields(&ws, &id, 1, "deleted", 0)).await;
    c.result(&result_fields(&ws, &id, 1, "indexed", 1)).await;
    let (status, eligible): (String, bool) = sqlx::query_as(
        "SELECT status, ai_eligible FROM workspace.workspace_documents WHERE id = $1",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(status, "archived");
    assert!(!eligible);
    let audits: Vec<(String,)> = sqlx::query_as(
        "SELECT action FROM workspace.workspace_document_audits WHERE document_id = $1 \
         ORDER BY action_at",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_all(&c.pool)
    .await
    .unwrap();
    let actions: Vec<&str> = audits.iter().map(|a| a.0.as_str()).collect();
    assert!(actions.contains(&"UploadDocument") && actions.contains(&"ArchiveDocument"));
    assert!(actions.contains(&"EmbeddingIndexed"));
}

#[tokio::test]
async fn index_results_update_status() {
    let Some(c) = ctx().await else { return };
    let owner = UserId::new();
    let ws = c.create_ws(owner).await;
    let status_of = |id: String| {
        let (c, ws) = (&c, ws.clone());
        async move {
            let (_, v) = c
                .call(
                    Method::GET,
                    &format!("/workspaces/{ws}/documents/{id}"),
                    owner,
                    None,
                )
                .await;
            v
        }
    };
    let doc = || async {
        let (_, v) = c.upload(&ws, owner, "d.txt", b"text", &[]).await;
        v["id"].as_str().unwrap().to_owned()
    };

    let a = doc().await;
    c.result(&result_fields(&ws, &a, 1, "indexed", 3)).await;
    let v = status_of(a.clone()).await;
    assert_eq!(v["ingestionStatus"], "completed");
    assert_eq!(v["aiEligible"], true);

    let b = doc().await;
    let mut f = result_fields(&ws, &b, 1, "blocked", 0);
    f.retain(|(k, _)| *k != "reason");
    f.push(("reason", "source_not_active".into()));
    c.result(&f).await;
    let v = status_of(b).await;
    assert_eq!(v["ingestionStatus"], "skipped");
    assert_eq!(v["ingestionFailureReason"], "source_not_active");

    let d = doc().await;
    let mut f = result_fields(&ws, &d, 1, "failed", 0);
    f.retain(|(k, _)| *k != "reason");
    f.push(("reason", "provider down".into()));
    c.result(&f).await;
    let v = status_of(d).await;
    assert_eq!(v["ingestionStatus"], "failed");
    assert_eq!(
        v["ingestionFailureReason"],
        "embedding_failed: provider down"
    );

    // Indexed with zero chunks (pdf awaiting extraction) is not "completed".
    let e = doc().await;
    c.result(&result_fields(&ws, &e, 1, "indexed", 0)).await;
    let v = status_of(e).await;
    assert_eq!(v["ingestionStatus"], "skipped");
    assert_eq!(v["ingestionFailureReason"], "no_indexable_content");
    assert_eq!(v["aiEligible"], false);

    // Other sources, unknown documents and wrong workspaces are acknowledged and ignored.
    let mut other = result_fields(&ws, &a, 1, "failed", 0);
    other.retain(|(k, _)| *k != "source_type");
    other.push(("source_type", "transcript".into()));
    c.result(&other).await;
    c.result(&result_fields(
        &ws,
        &Uuid::now_v7().to_string(),
        1,
        "indexed",
        1,
    ))
    .await;
    c.result(&result_fields(
        &Uuid::now_v7().to_string(),
        &a,
        1,
        "failed",
        0,
    ))
    .await;
    assert_eq!(status_of(a).await["ingestionStatus"], "completed");

    // Garbage is dead-lettered (Fatal), not retried.
    let bad = StreamMessage {
        stream: "embedding:index_results".into(),
        id: "0-2".into(),
        fields: [("status".to_owned(), "indexed".to_owned())].into(),
    };
    assert!(matches!(
        c.module.document_results_handler().handle(&bad).await,
        Err(warptalk_bus::HandlerError::Fatal(_))
    ));
}

#[tokio::test]
async fn documents_unavailable_without_storage() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return;
    };
    let pool = warptalk_db::connect(&url, 2).await.unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let module = WorkspaceModule::new(WorkspaceDeps {
        pool,
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        users: Arc::new(NoopUserDirectory),
    });
    let token = keys
        .issue_access(UserId::new(), Uuid::new_v4(), false, chrono::Utc::now())
        .unwrap()
        .0;
    let resp = module
        .router()
        .oneshot(
            Request::get(format!("/workspaces/{}/documents", Uuid::new_v4()))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
