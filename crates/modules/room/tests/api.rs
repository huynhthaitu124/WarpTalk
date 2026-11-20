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
use warptalk_kernel::{Clock, SystemClock, UserId, WorkspaceId, WorkspaceRole};
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
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
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
