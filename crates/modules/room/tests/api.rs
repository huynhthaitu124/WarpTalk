#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{Clock, FixedClock, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_room::application::ports::{InMemoryWorkspacePolicy, NoopVoiceConsentDirectory};
use warptalk_room::infrastructure::redis_publisher::RedisRoomEventPublisher;
use warptalk_room::{MIGRATIONS, RoomDeps, RoomModule};
use warptalk_web::JwtKeys;

struct Ctx {
    pool: PgPool,
    router: Router,
    module: RoomModule,
    keys: JwtKeys,
    policy: Arc<InMemoryWorkspacePolicy>,
    redis: redis::aio::ConnectionManager,
}

async fn ctx() -> Option<Ctx> {
    ctx_with(Arc::new(SystemClock)).await
}

async fn ctx_with(clock: Arc<dyn Clock>) -> Option<Ctx> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let policy = Arc::new(InMemoryWorkspacePolicy::default());
    let module = RoomModule::new(RoomDeps {
        pool: pool.clone(),
        jwt: keys.clone(),
        clock,
        workspace: policy.clone(),
        consent: Arc::new(NoopVoiceConsentDirectory),
        events: Arc::new(RedisRoomEventPublisher::new(redis.clone())),
    });
    Some(Ctx {
        pool,
        router: module.router(),
        module,
        keys,
        policy,
        redis,
    })
}

impl Ctx {
    async fn call(
        &self,
        m: Method,
        uri: &str,
        u: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let token = self
            .keys
            .issue_access(u, Uuid::new_v4(), false, chrono::Utc::now())
            .unwrap()
            .0;
        let mut b = Request::builder()
            .method(m)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn post(&self, uri: &str, u: UserId) -> (StatusCode, Value) {
        self.call(Method::POST, uri, u, None).await
    }

    async fn cached_routes(&mut self, room: &str) -> (Value, i64) {
        let key = format!("translationRoom:{room}:audio_routes");
        let raw: String = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut self.redis)
            .await
            .unwrap();
        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut self.redis)
            .await
            .unwrap();
        (serde_json::from_str(&raw).unwrap(), ttl)
    }

    async fn session_counts(&self, room: &str) -> (i64, i64) {
        let rid: Uuid = room.parse().unwrap();
        let q = |status: &'static str| {
            let pool = self.pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM translation_room.translation_room_sessions \
                     WHERE translation_room_id = $1 AND status = $2",
                )
                .bind(rid)
                .bind(status)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        (q("ACTIVE").await, q("ENDED").await)
    }
}

fn route_pairs(snapshot: &Value, parts: &[(&str, &str)]) -> Vec<(String, String, String, String)> {
    let name = |id: &str| parts.iter().find(|(i, _)| *i == id).unwrap().1.to_owned();
    let mut v: Vec<_> = snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                name(r["source_participant_id"].as_str().unwrap()),
                name(r["target_participant_id"].as_str().unwrap()),
                r["source_language"].as_str().unwrap().to_owned(),
                r["target_language"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    v.sort();
    v
}

fn s(a: &str, b: &str, c: &str, d: &str) -> (String, String, String, String) {
    (a.into(), b.into(), c.into(), d.into())
}

#[tokio::test]
async fn full_room_flow() {
    let Some(mut c) = ctx().await else { return };
    let (host, alice, bob, outsider) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let ws = WorkspaceId::new();
    for u in [host, alice, bob] {
        c.policy.set_role(ws, u, WorkspaceRole::Member);
    }
    let rooms = "/translation-rooms";

    // create: non-member refused, member ok
    let body = json!({"workspaceId": ws, "title": "Standup", "sourceLanguage": "vi", "targetLanguages": ["en"]});
    let (st, _) = c
        .call(Method::POST, rooms, outsider, Some(body.clone()))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c.call(Method::POST, rooms, host, Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["status"], "WAITING");
    assert_eq!(v["roomCode"].as_str().unwrap().len(), 12);
    let room = v["id"].as_str().unwrap().to_owned();
    let code = v["roomCode"].as_str().unwrap().to_owned();

    // scheduled in the future
    let later = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let (st, v) = c
        .call(
            Method::POST,
            rooms,
            host,
            Some(
                json!({"workspaceId": ws, "title": "Later", "sourceLanguage": "vi",
                        "targetLanguages": ["en"], "scheduledAt": later}),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["status"], "SCHEDULED");

    // list / detail / access
    let (st, v) = c
        .call(
            Method::GET,
            &format!("{rooms}?workspaceId={ws}&status=WAITING"),
            alice,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 1);
    let (st, _) = c
        .call(
            Method::GET,
            &format!("{rooms}?workspaceId={ws}"),
            outsider,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c
        .call(Method::GET, &format!("{rooms}/{room}"), outsider, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, v) = c
        .call(Method::GET, &format!("{rooms}/{room}"), alice, None)
        .await;
    assert_eq!(
        (st, v["roomCode"].as_str().unwrap()),
        (StatusCode::OK, code.as_str())
    );

    // alice joins by code: approval required -> WAITING
    let join = json!({"roomCode": code, "speakLanguage": "en", "listenLanguage": "en", "displayName": "Alice"});
    let (st, v) = c
        .call(Method::POST, &format!("{rooms}/join"), alice, Some(join))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "WAITING");
    let alice_pid = v["id"].as_str().unwrap().to_owned();
    // language outside the room is rejected
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{rooms}/{room}/join"),
            alice,
            Some(json!({"speakLanguage": "fr", "listenLanguage": "en"})),
        )
        .await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::BAD_REQUEST, "LANGUAGE_NOT_IN_ROOM")
    );

    // lifecycle is host only
    let (st, v) = c.post(&format!("{rooms}/{room}/start"), alice).await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::FORBIDDEN, "FORBIDDEN")
    );
    let (st, _) = c.post(&format!("{rooms}/{room}/start"), outsider).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // admit is host only, then works
    let admit = format!("{rooms}/{room}/participants/{alice_pid}/admit");
    let (st, _) = c.call(Method::PUT, &admit, alice, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c.call(Method::PUT, &admit, host, None).await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "CONNECTED")
    );
    let (st, _) = c.call(Method::PUT, &admit, host, None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, v) = c
        .call(
            Method::GET,
            &format!("{rooms}/{room}/participants"),
            host,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let host_pid = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["role"] == "HOST")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let parts = [(host_pid.as_str(), "host"), (alice_pid.as_str(), "alice")];

    // start: session opened, routes published to Redis (host vi/vi, alice en/en)
    let (st, v) = c.post(&format!("{rooms}/{room}/start"), host).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "IN_PROGRESS");
    assert_eq!(v["translationActive"], true);
    assert_eq!(c.session_counts(&room).await, (1, 0));
    let (snap, ttl) = c.cached_routes(&room).await;
    assert!(ttl > 43_000 && ttl <= 43_200, "ttl {ttl}");
    assert_eq!(snap["room_status"], "IN_PROGRESS");
    assert_eq!(snap["translation_active"], true);
    assert_eq!(snap["room_languages"], json!(["vi", "en"]));
    assert_eq!(
        route_pairs(&snap, &parts),
        vec![
            s("alice", "host", "en", "vi"),
            s("host", "alice", "vi", "en")
        ]
    );
    assert!(
        snap["routes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "PENDING")
    );
    // idempotent start keeps one session
    let (st, _) = c.post(&format!("{rooms}/{room}/start"), host).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(c.session_counts(&room).await, (1, 0));

    // GET routes mirrors the snapshot
    let (st, v) = c
        .call(
            Method::GET,
            &format!("{rooms}/{room}/audio-routes"),
            alice,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["routes"].as_array().unwrap().len(), 2);
    assert_eq!(v["translationActive"], true);

    // alice listens in vi: host->alice disappears, alice->host stays
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{rooms}/{room}/join"),
            alice,
            Some(json!({"speakLanguage": "en", "listenLanguage": "vi"})),
        )
        .await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "CONNECTED")
    );
    let (snap, _) = c.cached_routes(&room).await;
    assert_eq!(
        route_pairs(&snap, &parts),
        vec![s("alice", "host", "en", "vi")]
    );
    // alice back to en: route recreated (PENDING)
    c.call(
        Method::POST,
        &format!("{rooms}/{room}/join"),
        alice,
        Some(json!({"speakLanguage": "en", "listenLanguage": "en"})),
    )
    .await;
    let (st, v) = c
        .post(&format!("{rooms}/{room}/audio-routes/generate"), alice)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v2) = c
        .post(&format!("{rooms}/{room}/audio-routes/generate"), host)
        .await;
    assert_eq!(st, StatusCode::OK, "{v} {v2}");
    assert_eq!(v2["routes"].as_array().unwrap().len(), 2);

    // pause closes the session; resume opens a new one
    let (st, v) = c.post(&format!("{rooms}/{room}/pause"), host).await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "PAUSED")
    );
    assert_eq!(c.session_counts(&room).await, (0, 1));
    let (snap, _) = c.cached_routes(&room).await;
    assert_eq!(
        (
            snap["room_status"].as_str().unwrap(),
            snap["translation_active"].as_bool().unwrap()
        ),
        ("PAUSED", false)
    );
    let (st, _) = c.post(&format!("{rooms}/{room}/start"), host).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, v) = c.post(&format!("{rooms}/{room}/resume"), host).await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "IN_PROGRESS")
    );
    assert_eq!(c.session_counts(&room).await, (1, 1));

    // stop translation: room stays IN_PROGRESS, translation inactive
    let (st, v) = c
        .post(&format!("{rooms}/{room}/translation/stop"), host)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (
            v["status"].as_str().unwrap(),
            v["translationActive"].as_bool().unwrap()
        ),
        ("IN_PROGRESS", false)
    );
    assert_eq!(c.session_counts(&room).await, (0, 2));
    let (snap, _) = c.cached_routes(&room).await;
    assert_eq!(snap["translation_active"], false);
    let (st, v) = c.post(&format!("{rooms}/{room}/resume"), host).await;
    assert_eq!(
        (st, v["translationActive"].as_bool().unwrap()),
        (StatusCode::OK, true)
    );

    // kicked users cannot rejoin; host cannot be kicked
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{rooms}/join"),
            bob,
            Some(json!({"roomCode": code, "speakLanguage": "en", "listenLanguage": "en"})),
        )
        .await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "WAITING")
    );
    let bob_pid = v["id"].as_str().unwrap().to_owned();
    let (st, _) = c
        .call(
            Method::PUT,
            &format!("{rooms}/{room}/participants/{host_pid}/kick"),
            host,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let kick = format!("{rooms}/{room}/participants/{bob_pid}/kick");
    let (st, _) = c.call(Method::PUT, &kick, alice, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c.call(Method::PUT, &kick, host, None).await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "KICKED")
    );
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{rooms}/{room}/join"),
            bob,
            Some(json!({"speakLanguage": "en", "listenLanguage": "en"})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (st, _) = c
        .call(
            Method::POST,
            &format!("{rooms}/join"),
            bob,
            Some(json!({"roomCode": code, "speakLanguage": "en", "listenLanguage": "en"})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // alice leaves: her routes go away
    let (st, v) = c
        .call(
            Method::PUT,
            &format!("{rooms}/{room}/participants/me/leave"),
            alice,
            None,
        )
        .await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "LEFT")
    );
    let (snap, _) = c.cached_routes(&room).await;
    assert!(snap["routes"].as_array().unwrap().is_empty());

    // facade
    let rid = room.parse().unwrap();
    assert_eq!(
        c.module.queries().effective_host(rid).await.unwrap(),
        Some(host)
    );
    assert_eq!(
        c.module.queries().room_workspace(rid).await.unwrap(),
        Some(ws)
    );

    let summary = c.module.queries().room_summary(rid).await.unwrap().unwrap();
    assert_eq!(
        (
            summary.workspace_id,
            summary.host_id,
            summary.effective_host_id
        ),
        (ws, host, host)
    );
    assert!(!summary.status.is_empty());
    let p = c
        .module
        .queries()
        .participant_summary(rid, host)
        .await
        .unwrap();
    assert!(p.is_some());
    assert!(
        c.module
            .queries()
            .participant_summary(rid, outsider)
            .await
            .unwrap()
            .is_none()
    );

    // end (host only), idempotent, then the room is closed
    let (st, _) = c.post(&format!("{rooms}/{room}/end"), alice).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c.post(&format!("{rooms}/{room}/end"), host).await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "ENDED")
    );
    assert_eq!(c.session_counts(&room).await.0, 0);
    let (st, _) = c.post(&format!("{rooms}/{room}/end"), host).await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = c
        .call(
            Method::POST,
            &format!("{rooms}/{room}/join"),
            alice,
            Some(json!({"speakLanguage": "en", "listenLanguage": "en"})),
        )
        .await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "INVALID_STATE")
    );
    let (st, _) = c.post(&format!("{rooms}/{room}/cancel"), host).await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn cancel_and_inactive_workspace() {
    let Some(c) = ctx().await else { return };
    let (host, other) = (UserId::new(), UserId::new());
    let ws = WorkspaceId::new();
    c.policy.set_role(ws, host, WorkspaceRole::Owner);
    c.policy.set_role(ws, other, WorkspaceRole::Admin);
    let (_, v) = c
        .call(
            Method::POST,
            "/translation-rooms",
            host,
            Some(
                json!({"workspaceId": ws, "title": "T", "sourceLanguage": "en",
                        "targetLanguages": ["vi"], "requiresApproval": false}),
            ),
        )
        .await;
    let room = v["id"].as_str().unwrap().to_owned();
    // workspace suspended: start blocked
    c.policy.set_active(ws, false);
    let (st, v) = c
        .post(&format!("/translation-rooms/{room}/start"), host)
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    c.policy.set_active(ws, true);
    // generate with no one but host: ok; cancel from WAITING by host
    let (st, v) = c
        .post(&format!("/translation-rooms/{room}/cancel"), host)
        .await;
    assert_eq!(
        (st, v["status"].as_str().unwrap()),
        (StatusCode::OK, "CANCELLED")
    );
    let (st, v) = c
        .post(&format!("/translation-rooms/{room}/start"), host)
        .await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "INVALID_STATE")
    );
    let (st, _) = c
        .post(
            "/translation-rooms/00000000-0000-0000-0000-000000000000/start",
            host,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ external bridge (D2)

fn meet_code() -> String {
    let h = Uuid::new_v4().simple().to_string();
    let l: Vec<char> = h
        .chars()
        .map(|c| (b'a' + (c.to_digit(16).unwrap() as u8)) as char)
        .collect();
    let s: String = l.iter().collect();
    format!("{}-{}-{}", &s[0..3], &s[3..7], &s[7..10])
}

fn claim_body(ws: WorkspaceId, code: &str) -> Value {
    json!({"workspaceId": ws, "provider": "google_meet", "meetingUrl": format!("https://meet.google.com/{code}"),
           "sourceLanguage": "vi", "targetLanguages": ["en"]})
}

const CLAIM: &str = "/translation-rooms/bridge/claim";

fn bridge_uri(room: &str, tail: &str) -> String {
    format!("/translation-rooms/{room}/bridge/{tail}")
}

/// Workspace with `n` members.
fn members(c: &Ctx, n: usize) -> (WorkspaceId, Vec<UserId>) {
    let ws = WorkspaceId::new();
    let users: Vec<UserId> = (0..n).map(|_| UserId::new()).collect();
    for u in &users {
        c.policy.set_role(ws, *u, WorkspaceRole::Member);
    }
    (ws, users)
}

#[tokio::test]
async fn bridge_claim_is_idempotent_per_meeting() {
    let Some(c) = ctx().await else { return };
    let (ws, u) = members(&c, 2);
    let (a, b) = (u[0], u[1]);
    let code = meet_code();

    let (st, v) = c
        .call(Method::POST, CLAIM, a, Some(claim_body(ws, &code)))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["created"], true);
    assert_eq!(v["bridgeRole"], "capturer");
    assert_eq!(v["roomType"], "EXTERNAL_BRIDGE");
    assert_eq!(v["bridge"]["provider"], "google_meet");
    assert_eq!(v["bridge"]["meetingCode"], code);
    assert_eq!(v["bridge"]["capturerUserId"], a.to_string());
    assert_eq!(v["bridge"]["audioMode"], "voice");
    assert_eq!(v["heartbeatIntervalSeconds"], 15);
    assert_eq!(v["leaseSeconds"], 45);
    let room = v["id"].as_str().unwrap().to_owned();

    // same meeting (bare code, other member): same room, not capturer
    let body = json!({"workspaceId": ws, "provider": "google_meet", "code": code.to_uppercase(),
                      "sourceLanguage": "vi", "targetLanguages": ["en"]});
    let (st, v) = c.call(Method::POST, CLAIM, b, Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["id"], room.as_str());
    assert_eq!(v["created"], false);
    assert_eq!(v["bridgeRole"], "viewer");
    assert_eq!(v["bridge"]["capturerUserId"], a.to_string());

    // the capturer re-claiming keeps the role
    let (st, v) = c
        .call(Method::POST, CLAIM, a, Some(claim_body(ws, &code)))
        .await;
    assert_eq!(
        (
            st,
            v["id"].as_str().unwrap(),
            v["bridgeRole"].as_str().unwrap()
        ),
        (StatusCode::OK, room.as_str(), "capturer")
    );

    // different meeting: different room
    let (st, v) = c
        .call(Method::POST, CLAIM, a, Some(claim_body(ws, &meet_code())))
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_ne!(v["id"], room.as_str());

    // stand-in seat is in the roster
    let (st, v) = c
        .call(
            Method::GET,
            &format!("/translation-rooms/{room}/participants"),
            a,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let standin = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["userId"] == "00000000-0000-0000-0000-00000000b21d")
        .expect("stand-in participant");
    assert_eq!(standin["displayName"], "External Meeting");

    // validation and access
    let outsider = UserId::new();
    let (st, _) = c
        .call(Method::POST, CLAIM, outsider, Some(claim_body(ws, &code)))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let mut bad = claim_body(ws, &code);
    bad["meetingUrl"] = json!("https://example.com/nope");
    let (st, v) = c.call(Method::POST, CLAIM, a, Some(bad)).await;
    assert!(st.is_client_error(), "{v}");
    assert_eq!(v["code"], "INVALID_MEETING_REF");
    let mut bad = claim_body(ws, &code);
    bad["provider"] = json!("skype");
    let (_, v) = c.call(Method::POST, CLAIM, a, Some(bad)).await;
    assert_eq!(v["code"], "INVALID_PROVIDER");
}

#[tokio::test]
async fn bridge_concurrent_claims_yield_one_room_and_unique_index_guards() {
    let Some(c) = ctx().await else { return };
    let (ws, u) = members(&c, 2);
    let code = meet_code();
    let (r1, r2) = tokio::join!(
        c.call(Method::POST, CLAIM, u[0], Some(claim_body(ws, &code))),
        c.call(Method::POST, CLAIM, u[1], Some(claim_body(ws, &code))),
    );
    assert!(r1.0.is_success() && r2.0.is_success(), "{} {}", r1.1, r2.1);
    assert_eq!(r1.1["id"], r2.1["id"]);
    assert_eq!(
        [
            r1.1["created"].as_bool().unwrap(),
            r2.1["created"].as_bool().unwrap()
        ]
        .iter()
        .filter(|c| **c)
        .count(),
        1
    );
    let room = r1.1["id"].as_str().unwrap().to_owned();

    // The partial unique index rejects a second open row; the repo maps it to the Conflict text.
    let dup = sqlx::query(
        "INSERT INTO translation_room.translation_rooms \
         (id, workspace_id, host_id, title, translation_room_code, status, translation_room_type, \
          source_language, created_at, bridge_provider, bridge_meeting_code) \
         VALUES ($1, $2, $3, 'dup', $4, 'WAITING', 'EXTERNAL_BRIDGE', 'vi', now(), 'google_meet', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(ws.as_uuid())
    .bind(u[0].as_uuid())
    .bind(&Uuid::new_v4().simple().to_string()[..12])
    .bind(&code)
    .execute(&c.pool)
    .await
    .unwrap_err();
    assert!(warptalk_room::infrastructure::pg_repo::is_open_bridge_violation(&dup));
    assert_eq!(
        warptalk_room::infrastructure::pg_repo::OPEN_BRIDGE_CONFLICT,
        "An open room for this external meeting already exists."
    );

    // Once the room ended, the same meeting can be claimed again (new room).
    let (st, _) = c
        .call(
            Method::POST,
            &format!("/translation-rooms/{room}/end"),
            u[0],
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = c
        .call(Method::POST, CLAIM, u[1], Some(claim_body(ws, &code)))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_ne!(v["id"], room.as_str());
}

/// Claim at a fixed clock; returns (ctx, clock, ws, [capturer/host, other, third], room id).
async fn clocked_bridge() -> Option<(Ctx, Arc<FixedClock>, WorkspaceId, Vec<UserId>, String)> {
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let c = ctx_with(clock.clone()).await?;
    let (ws, u) = members(&c, 3);
    let (st, v) = c
        .call(
            Method::POST,
            CLAIM,
            u[0],
            Some(claim_body(ws, &meet_code())),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let room = v["id"].as_str().unwrap().to_owned();
    Some((c, clock, ws, u, room))
}

fn advance(clock: &FixedClock, secs: i64) {
    clock.set(clock.now() + chrono::Duration::seconds(secs));
}

fn rid(room: &str) -> RoomId {
    room.parse::<Uuid>().unwrap().into()
}

#[tokio::test]
async fn bridge_heartbeat_keeps_lease() {
    let Some((c, clock, _ws, u, room)) = clocked_bridge().await else {
        return;
    };
    let hb = bridge_uri(&room, "capturer/heartbeat");
    let take = bridge_uri(&room, "capturer/takeover");
    for _ in 0..3 {
        advance(&clock, 30);
        let (st, v) = c
            .call(Method::POST, &hb, u[0], Some(json!({"audioFlowing": true})))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["bridge"]["audioFlowing"], true);
    }
    // 90 s after the claim, but only 0 s after the last heartbeat: not stale
    let info = c
        .module
        .queries()
        .bridge_info(rid(&room))
        .await
        .unwrap()
        .unwrap();
    assert!(!info.capturer_stale);
    assert_eq!(info.capturer_user_id, Some(u[0]));
    assert_eq!(info.provider, "google_meet");
    assert_eq!(info.audio_mode, "voice");
    advance(&clock, 30);
    let (st, v) = c.call(Method::POST, &take, u[1], None).await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "CAPTURER_ACTIVE")
    );
    // a non-capturer cannot heartbeat
    let (st, v) = c.call(Method::POST, &hb, u[1], None).await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "CAPTURER_LEASE_LOST")
    );
    // body is optional (older clients)
    let (st, _) = c.call(Method::POST, &hb, u[0], None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn bridge_takeover_when_lease_stale() {
    let Some((c, clock, _ws, u, room)) = clocked_bridge().await else {
        return;
    };
    let hb = bridge_uri(&room, "capturer/heartbeat");
    let take = bridge_uri(&room, "capturer/takeover");
    advance(&clock, 45);
    let info = c
        .module
        .queries()
        .bridge_info(rid(&room))
        .await
        .unwrap()
        .unwrap();
    assert!(!info.capturer_stale, "exactly at the window is still fresh");
    let (st, _) = c.call(Method::POST, &take, u[1], None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    advance(&clock, 1);
    assert!(
        c.module
            .queries()
            .bridge_info(rid(&room))
            .await
            .unwrap()
            .unwrap()
            .capturer_stale
    );
    let (st, v) = c.call(Method::POST, &take, u[1], None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["bridge"]["capturerUserId"], u[1].to_string());
    assert_eq!(v["bridge"]["epoch"], 2);
    // the old capturer lost the lease; the new one holds it
    let (st, v) = c.call(Method::POST, &hb, u[0], None).await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "CAPTURER_LEASE_LOST")
    );
    assert_eq!(
        c.call(Method::POST, &hb, u[1], None).await.0,
        StatusCode::OK
    );
    // the host may take over even though the lease is fresh
    let (st, v) = c.call(Method::POST, &take, u[0], None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["bridge"]["capturerUserId"], u[0].to_string());
    // outsiders do not even see the room
    let (st, _) = c.call(Method::POST, &take, UserId::new(), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bridge_silent_audio_makes_lease_stale_despite_heartbeats() {
    let Some((c, clock, _ws, u, room)) = clocked_bridge().await else {
        return;
    };
    let hb = bridge_uri(&room, "capturer/heartbeat");
    let take = bridge_uri(&room, "capturer/takeover");
    let q = c.module.queries();
    // heartbeats every 15 s, audio never flows
    for _ in 0..3 {
        advance(&clock, 15);
        let (st, v) = c
            .call(
                Method::POST,
                &hb,
                u[0],
                Some(json!({"audioFlowing": false})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(
            !q.bridge_info(rid(&room))
                .await
                .unwrap()
                .unwrap()
                .capturer_stale
        );
    }
    // t = 45: the fourth heartbeat is still within the window, the fifth is not
    advance(&clock, 15);
    c.call(
        Method::POST,
        &hb,
        u[0],
        Some(json!({"audioFlowing": false})),
    )
    .await;
    assert!(
        q.bridge_info(rid(&room))
            .await
            .unwrap()
            .unwrap()
            .capturer_stale
    );
    let (st, v) = c.call(Method::POST, &take, u[1], None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["bridge"]["capturerUserId"], u[1].to_string());
}

#[tokio::test]
async fn bridge_audio_mode_locks_while_translation_runs() {
    let Some(c) = ctx().await else { return };
    let (ws, u) = members(&c, 2);
    let (st, v) = c
        .call(
            Method::POST,
            CLAIM,
            u[0],
            Some(claim_body(ws, &meet_code())),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let room = v["id"].as_str().unwrap().to_owned();
    let uri = bridge_uri(&room, "audio-mode");
    let put =
        |user: UserId, mode: &str| c.call(Method::PUT, &uri, user, Some(json!({"mode": mode})));

    let (st, v) = put(u[1], "text").await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (_, v) = put(u[0], "loud").await;
    assert_eq!(v["code"], "INVALID_AUDIO_MODE");

    let (st, v) = put(u[0], "text").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["bridge"]["audioMode"], "text");
    // not running yet: back to voice is fine, then text again
    assert_eq!(put(u[0], "voice").await.0, StatusCode::OK);
    assert_eq!(put(u[0], "text").await.0, StatusCode::OK);

    let (st, v) = c
        .post(&format!("/translation-rooms/{room}/start"), u[0])
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["translationActive"], true);
    let (st, v) = put(u[0], "voice").await;
    assert_eq!(
        (st, v["code"].as_str().unwrap()),
        (StatusCode::CONFLICT, "BRIDGE_AUDIO_MODE_LOCKED")
    );
    // voice to text stays allowed while running
    let (st, _) = c
        .post(&format!("/translation-rooms/{room}/translation/stop"), u[0])
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(put(u[0], "voice").await.0, StatusCode::OK);
}

#[tokio::test]
async fn bridge_endpoints_refuse_native_rooms() {
    let Some(c) = ctx().await else { return };
    let (ws, u) = members(&c, 1);
    let (st, v) = c
        .call(
            Method::POST,
            "/translation-rooms",
            u[0],
            Some(json!({"workspaceId": ws, "title": "Native", "sourceLanguage": "vi", "targetLanguages": ["en"]})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["roomType"], "NATIVE");
    assert!(v.get("bridge").is_none());
    let room = v["id"].as_str().unwrap().to_owned();
    for (m, tail, body) in [
        (Method::POST, "capturer/heartbeat", None),
        (Method::POST, "capturer/takeover", None),
        (Method::PUT, "audio-mode", Some(json!({"mode": "text"}))),
    ] {
        let (st, v) = c.call(m, &bridge_uri(&room, tail), u[0], body).await;
        assert_eq!(
            (st, v["code"].as_str().unwrap()),
            (StatusCode::CONFLICT, "NOT_BRIDGE_ROOM"),
            "{tail}"
        );
    }
    assert!(
        c.module
            .queries()
            .bridge_info(rid(&room))
            .await
            .unwrap()
            .is_none()
    );
}
