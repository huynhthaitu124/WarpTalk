#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use futures::StreamExt;
use redis::aio::ConnectionManager;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_transcript::application::glossary_ports::InMemoryRoles;
use warptalk_transcript::application::ports::{
    AlwaysSaveRoomPolicy, InMemoryReadAccess, InMemoryRooms, StaticAiPolicy,
};
use warptalk_transcript::infrastructure::redis_adapters::GLOSSARY_CHANGED_CHANNEL;
use warptalk_transcript::{MIGRATIONS, TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;

struct Ctx {
    pool: PgPool,
    redis: ConnectionManager,
    redis_url: String,
    router: Router,
    keys: JwtKeys,
    ws: WorkspaceId,
    owner: String,
    admin: String,
    member: String,
    outsider: String,
    sysadmin: String,
}

fn token(keys: &JwtKeys, user: UserId, sa: bool) -> String {
    keys.issue_access(user, Uuid::new_v4(), sa, Utc::now())
        .unwrap()
        .0
}

async fn ctx() -> Option<Ctx> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let ws = WorkspaceId::new();
    let (owner, admin, member, outsider, sysadmin) = (
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
    );
    let roles = Arc::new(InMemoryRoles::default());
    roles.set(ws, owner, WorkspaceRole::Owner);
    roles.set(ws, admin, WorkspaceRole::Admin);
    roles.set(ws, member, WorkspaceRole::Member);
    let rooms = Arc::new(InMemoryRooms::default());
    let module = TranscriptModule::new(TranscriptDeps {
        pool: pool.clone(),
        redis: redis.clone(),
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        rooms: rooms.clone(),
        room_policy: Arc::new(AlwaysSaveRoomPolicy),
        workspace_ai: Arc::new(StaticAiPolicy {
            retrieval: false,
            external_llm: true,
        }),
        host: rooms,
        read_access: Arc::new(InMemoryReadAccess::default()),
    })
    .with_workspace_roles(roles);
    Some(Ctx {
        pool,
        redis,
        redis_url,
        router: module.router(),
        ws,
        owner: token(&keys, owner, false),
        admin: token(&keys, admin, false),
        member: token(&keys, member, false),
        outsider: token(&keys, outsider, false),
        sysadmin: token(&keys, sysadmin, true),
        keys,
    })
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: Body,
    bearer: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"));
    if let Some(c) = content_type {
        req = req.header("content-type", c);
    }
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: &str,
) -> (StatusCode, Value) {
    let (status, text) = match body {
        Some(b) => {
            raw(
                app,
                method,
                uri,
                Some("application/json"),
                Body::from(b.to_string()),
                bearer,
            )
            .await
        }
        None => raw(app, method, uri, None, Body::empty(), bearer).await,
    };
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn snapshot(redis: &ConnectionManager, key: &str) -> Value {
    let mut conn = redis.clone();
    let s: Option<String> = redis::cmd("GET")
        .arg(key)
        .query_async(&mut conn)
        .await
        .unwrap();
    serde_json::from_str(&s.unwrap_or_else(|| "null".into())).unwrap()
}

type Changes = std::pin::Pin<Box<dyn futures::Stream<Item = (String, String)> + Send>>;

async fn listen(url: &str) -> Changes {
    let mut ps = redis::Client::open(url)
        .unwrap()
        .get_async_pubsub()
        .await
        .unwrap();
    ps.subscribe(GLOSSARY_CHANGED_CHANNEL).await.unwrap();
    Box::pin(ps.into_on_message().map(|m| {
        (
            m.get_channel_name().to_owned(),
            m.get_payload::<String>().unwrap(),
        )
    }))
}

/// Next change message for `want_id` (workspace id, or None for global).
async fn next_change(rx: &mut Changes, want_id: Option<String>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, payload) = rx.next().await.unwrap();
            let v: Value = serde_json::from_str(&payload).unwrap();
            let id = v["id"].as_str().map(str::to_owned);
            if id == want_id {
                return v;
            }
        }
    })
    .await
    .expect("no glossary change published")
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn workspace_glossary_crud_rbac_and_snapshot() {
    let Some(c) = ctx().await else { return };
    let ws = c.ws.to_string();
    let mut rx = listen(&c.redis_url).await;

    let body = json!({"workspaceId": ws, "name": uniq("Tech"), "sourceLanguage": "en", "targetLanguage": "vi"});
    // RBAC: member and outsider cannot write; invalid input.
    let (s, _) = call(
        &c.router,
        "POST",
        "/glossaries",
        Some(body.clone()),
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &c.router,
        "POST",
        "/glossaries",
        Some(body.clone()),
        &c.outsider,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut bad = body.clone();
    bad["sourceLanguage"] = json!("english");
    let (s, b) = call(&c.router, "POST", "/glossaries", Some(bad), &c.owner).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["code"], "INVALID_LANGUAGE");
    let mut same = body.clone();
    same["targetLanguage"] = json!("en");
    let (s, _) = call(&c.router, "POST", "/glossaries", Some(same), &c.owner).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, g) = call(
        &c.router,
        "POST",
        "/glossaries",
        Some(body.clone()),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let gid = g["id"].as_str().unwrap().to_owned();
    assert_eq!(g["termCount"], 0);
    next_change(&mut rx, Some(ws.clone())).await; // glossary creation
    let (s, b) = call(&c.router, "POST", "/glossaries", Some(body), &c.admin).await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["code"], "DUPLICATE_NAME");

    // Reads: member yes, outsider no.
    let (s, b) = call(
        &c.router,
        "GET",
        &format!("/glossaries/workspace/{ws}"),
        None,
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        b.as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == gid.as_str())
    );
    let (s, _) = call(
        &c.router,
        "GET",
        &format!("/glossaries/workspace/{ws}"),
        None,
        &c.outsider,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}"),
        None,
        &c.outsider,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{}", Uuid::new_v4()),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Terms.
    let terms = format!("/glossaries/{gid}/terms");
    let t1 = json!({"sourceTerm": "WarpTalk", "targetTerm": "WarpTalk", "caseSensitive": true, "notes": "brand"});
    let (s, _) = call(&c.router, "POST", &terms, Some(t1.clone()), &c.member).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, t) = call(&c.router, "POST", &terms, Some(t1), &c.admin).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let tid = t["id"].as_str().unwrap().to_owned();
    assert_eq!(t["notes"], "brand");

    let change = next_change(&mut rx, Some(ws.clone())).await;
    assert_eq!(change["scope"], "workspace");
    let snap = snapshot(&c.redis, &format!("glossary:workspace:{ws}")).await;
    assert_eq!(snap["version"], change["version"]);
    let listed = snap["terms"].as_array().unwrap();
    assert!(listed.iter().any(|t| t["source"] == "WarpTalk"
        && t["target"] == "WarpTalk"
        && t["sourceLang"] == "en"
        && t["targetLang"] == "vi"
        && t["caseSensitive"] == true));

    let dup = json!({"sourceTerm": "warptalk", "targetTerm": "x"});
    let (s, b) = call(&c.router, "POST", &terms, Some(dup), &c.owner).await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["code"], "DUPLICATE_TERM");
    let long = json!({"sourceTerm": "a".repeat(201), "targetTerm": "x"});
    let (s, b) = call(&c.router, "POST", &terms, Some(long), &c.owner).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALUE_TOO_LONG");
    let (s, _) = call(
        &c.router,
        "POST",
        &terms,
        Some(json!({"sourceTerm": " ", "targetTerm": "x"})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Update + list + count.
    let upd = json!({"sourceTerm": "WarpTalk", "targetTerm": "WarpTalk VN"});
    let (s, b) = call(
        &c.router,
        "PUT",
        &format!("{terms}/{tid}"),
        Some(upd),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["caseSensitive"], false);
    next_change(&mut rx, Some(ws.clone())).await;
    let snap = snapshot(&c.redis, &format!("glossary:workspace:{ws}")).await;
    assert!(
        snap["terms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target"] == "WarpTalk VN")
    );
    let (_, b) = call(&c.router, "GET", &terms, None, &c.member).await;
    assert_eq!(b.as_array().unwrap().len(), 1);
    let (_, b) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}"),
        None,
        &c.member,
    )
    .await;
    assert_eq!(b["termCount"], 1);

    // Rename (PUT) and member cannot.
    let (s, _) = call(
        &c.router,
        "PUT",
        &format!("/glossaries/{gid}"),
        Some(json!({"name": "x"})),
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let new_name = uniq("Renamed");
    let (s, b) = call(
        &c.router,
        "PUT",
        &format!("/glossaries/{gid}"),
        Some(json!({"name": new_name, "targetLanguage": "ja"})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["targetLanguage"], "ja");
    next_change(&mut rx, Some(ws.clone())).await;

    // Delete term then glossary: snapshot empties.
    let (s, _) = call(
        &c.router,
        "DELETE",
        &format!("{terms}/{tid}"),
        None,
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &c.router,
        "DELETE",
        &format!("{terms}/{tid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &c.router,
        "DELETE",
        &format!("{terms}/{tid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    next_change(&mut rx, Some(ws.clone())).await;
    let (_, b) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(b["termCount"], 0);
    let (s, _) = call(
        &c.router,
        "DELETE",
        &format!("/glossaries/{gid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let snap = snapshot(&c.redis, &format!("glossary:workspace:{ws}")).await;
    assert!(snap["terms"].as_array().unwrap().is_empty());
    let _ = &c.keys;
}

#[tokio::test]
async fn csv_import_dedupe_limit_and_export() {
    let Some(c) = ctx().await else { return };
    let ws = c.ws.to_string();
    let (s, g) = call(
        &c.router,
        "POST",
        "/glossaries",
        Some(json!({"workspaceId": ws, "name": uniq("Csv"), "sourceLanguage": "en", "targetLanguage": "vi"})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let gid = g["id"].as_str().unwrap();
    let bulk = format!("/glossaries/{gid}/terms/bulk");

    let csv = "source,target\nhello,xin chao\nHELLO,again\n\"a,b\",\"c\"\"d\"\n\nworld,the gioi\n";
    let (s, text) = raw(
        &c.router,
        "POST",
        &bulk,
        Some("text/csv"),
        Body::from(csv),
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{text}");
    let (s, text) = raw(
        &c.router,
        "POST",
        &bulk,
        Some("text/csv"),
        Body::from(csv),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{text}");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (r["imported"].as_u64(), r["skipped"].as_u64()),
        (Some(3), Some(1))
    );

    // JSON body; existing terms are skipped.
    let (s, r) = call(
        &c.router,
        "POST",
        &bulk,
        Some(json!({"csv": "Hello,x\nnew,moi\n"})),
        &c.admin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(
        (r["imported"].as_u64(), r["skipped"].as_u64()),
        (Some(1), Some(1))
    );

    // Invalid row rejects the whole payload.
    let (s, r) = call(
        &c.router,
        "POST",
        &bulk,
        Some(json!({"csv": "ok1,fine\nbroken-line\n"})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    assert_eq!(r["code"], "INVALID_CSV");

    // Limit: 2001 rows rejected, 2000 accepted.
    let rows = |n: usize| {
        (0..n)
            .map(|i| format!("bulk{i},t{i}\n"))
            .collect::<String>()
    };
    let (s, r) = call(
        &c.router,
        "POST",
        &bulk,
        Some(json!({"csv": rows(2001)})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    assert_eq!(r["code"], "IMPORT_TOO_LARGE");
    let (s, r) = call(
        &c.router,
        "POST",
        &bulk,
        Some(json!({"csv": rows(2000)})),
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["imported"], 2000);

    let (_, b) = call(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}"),
        None,
        &c.member,
    )
    .await;
    assert_eq!(b["termCount"], 2004);

    // Export round-trips quoting; readable by members, not outsiders.
    let (s, _) = raw(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}/export"),
        None,
        Body::empty(),
        &c.outsider,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, text) = raw(
        &c.router,
        "GET",
        &format!("/glossaries/{gid}/export"),
        None,
        Body::empty(),
        &c.member,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(text.starts_with("source,target\n"));
    assert!(text.contains("\"a,b\",\"c\"\"d\"\n"));
    assert!(text.contains("hello,xin chao\n"));
    assert_eq!(text.lines().count(), 2005);

    // Cleanup keeps the snapshot small for later runs.
    let (s, _) = call(
        &c.router,
        "DELETE",
        &format!("/glossaries/{gid}"),
        None,
        &c.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn global_glossary_audit_publish_cap_and_snapshot() {
    let Some(c) = ctx().await else { return };
    let mut rx = listen(&c.redis_url).await;
    let base = "/admin/global-glossary";

    let term = |t: &str| json!({"term": t, "preferredTranslation": "x", "targetLanguage": "vi", "businessDomain": "it", "definition": "d"});

    // System admin only.
    let (s, _) = call(&c.router, "GET", base, None, &c.owner).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&c.router, "POST", base, Some(term("nope")), &c.owner).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let name = uniq("Kubernetes");
    let (s, t) = call(
        &c.router,
        "POST",
        base,
        Some(json!({"term": name, "preferredTranslation": "K8s", "targetLanguage": "vi"})),
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    assert_eq!(t["status"], "draft");
    let id = t["id"].as_str().unwrap().to_owned();
    let (s, b) = call(&c.router, "POST", base, Some(json!({"term": name.to_uppercase(), "preferredTranslation": "other", "targetLanguage": "vi"})), &c.sysadmin).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["code"], "DUPLICATE_TERM");

    // Publishing needs a definition.
    let (s, b) = call(
        &c.router,
        "POST",
        &format!("{base}/{id}/publish"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["code"], "DEFINITION_REQUIRED");
    let upd = json!({"term": name, "preferredTranslation": "K8s", "targetLanguage": "vi", "definition": "Container orchestrator"});
    let (s, t) = call(
        &c.router,
        "PUT",
        &format!("{base}/{id}"),
        Some(upd),
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(t["version"], 2);

    // Draft is not in the global snapshot or the published list.
    let (_, list) = call(&c.router, "GET", "/glossaries/global", None, &c.member).await;
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == id.as_str())
    );
    let (s, t) = call(
        &c.router,
        "POST",
        &format!("{base}/{id}/publish"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(t["status"], "published");
    let change = next_change(&mut rx, None).await;
    assert_eq!(change["scope"], "global");
    let snap = snapshot(&c.redis, "glossary:global").await;
    assert!(snap["version"].as_i64().unwrap() >= change["version"].as_i64().unwrap());
    assert!(
        snap["terms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["source"] == name.as_str()
                && x["target"] == "K8s"
                && x["targetLang"] == "vi"
                && x["sourceLang"].is_null())
    );
    let (_, list) = call(&c.router, "GET", "/glossaries/global", None, &c.member).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == id.as_str())
    );

    // Archive removes it from the snapshot.
    let (s, t) = call(
        &c.router,
        "POST",
        &format!("{base}/{id}/archive"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(t["status"], "archived");
    next_change(&mut rx, None).await;
    let snap = snapshot(&c.redis, "glossary:global").await;
    assert!(
        !snap["terms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["source"] == name.as_str())
    );

    // Paged list with status filter.
    let (s, page) = call(
        &c.router,
        "GET",
        &format!("{base}?status=archived&pageSize=5"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert!(page["total"].as_i64().unwrap() >= 1);
    let (s, _) = call(
        &c.router,
        "GET",
        &format!("{base}?status=bogus"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Cap of 200 published terms.
    let (_, page) = call(
        &c.router,
        "GET",
        &format!("{base}?status=published&pageSize=1"),
        None,
        &c.sysadmin,
    )
    .await;
    let published = page["total"].as_i64().unwrap();
    let mut created = vec![id.clone()];
    let mut to_publish = (200 - published).max(0);
    // The archived term above is republished first, then fresh ones fill the cap.
    let (s, _) = call(
        &c.router,
        "POST",
        &format!("{base}/{id}/publish"),
        None,
        &c.sysadmin,
    )
    .await;
    if to_publish > 0 {
        assert_eq!(s, StatusCode::OK);
        to_publish -= 1;
    } else {
        assert_eq!(s, StatusCode::CONFLICT);
    }
    for i in 0..to_publish {
        let (s, t) = call(
            &c.router,
            "POST",
            base,
            Some(term(&format!("{}-{i}", uniq("cap")))),
            &c.sysadmin,
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{t}");
        let tid = t["id"].as_str().unwrap().to_owned();
        created.push(tid.clone());
        let (s, b) = call(
            &c.router,
            "POST",
            &format!("{base}/{tid}/publish"),
            None,
            &c.sysadmin,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }
    let (s, t) = call(
        &c.router,
        "POST",
        base,
        Some(term(&uniq("over"))),
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let over = t["id"].as_str().unwrap().to_owned();
    created.push(over.clone());
    let (s, b) = call(
        &c.router,
        "POST",
        &format!("{base}/{over}/publish"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["code"], "GLOBAL_CAP_REACHED");
    let snap = snapshot(&c.redis, "glossary:global").await;
    assert!(snap["terms"].as_array().unwrap().len() <= 200);

    // Audit trail of the first term: created, updated, published, archived, published.
    let (s, audits) = call(
        &c.router,
        "GET",
        &format!("{base}/{id}/audits"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{audits}");
    let actions: Vec<&str> = audits
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        &actions[..4],
        ["created", "updated", "published", "archived"]
    );
    assert!(audits[0]["beforeJson"].is_null());
    assert_eq!(audits[0]["afterJson"]["term"], name.as_str());
    assert_eq!(audits[1]["beforeJson"]["version"], 1);
    assert_eq!(audits[1]["afterJson"]["version"], 2);

    // Delete writes a final audit that outlives the term; clean up everything created here.
    for tid in &created {
        let (s, _) = call(
            &c.router,
            "DELETE",
            &format!("{base}/{tid}"),
            None,
            &c.sysadmin,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (s, audits) = call(
        &c.router,
        "GET",
        &format!("{base}/{id}/audits"),
        None,
        &c.sysadmin,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        audits.as_array().unwrap().last().unwrap()["action"],
        "deleted"
    );
    let (s, _) = call(&c.router, "GET", &format!("{base}/{id}"), None, &c.sysadmin).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript.global_glossary_audits WHERE term_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert!(n >= 6);
}
