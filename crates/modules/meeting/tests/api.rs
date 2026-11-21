#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{Clock, RoomId, SystemClock, UserId, WorkspaceId};
use warptalk_livekit::{
    InMemoryRoomService, LiveKitConfig, RoomServiceCall, TokenClaims, sign_webhook,
};
use warptalk_meeting::application::ports::{
    BridgeInfo, EVENT_BILLING_STOP, EVENT_CHAT_HIDDEN, EVENT_CHAT_TRANSLATE, EVENT_END_ROOM,
    InMemoryMeetingEventPublisher, InMemoryRoomDirectory, RoomInfo,
};
use warptalk_meeting::{DEFAULT_TOKEN_TTL, MIGRATIONS, MeetingDeps, MeetingModule};
use warptalk_web::JwtKeys;

const SECRET: &str = "lk-secret-lk-secret-lk-secret-0001";

struct Ctx {
    pool: PgPool,
    router: Router,
    keys: JwtKeys,
    dir: Arc<InMemoryRoomDirectory>,
    lk: Arc<InMemoryRoomService>,
    events: Arc<InMemoryMeetingEventPublisher>,
    cfg: LiveKitConfig,
}

fn lk_cfg(secret: &str) -> LiveKitConfig {
    LiveKitConfig {
        url: "wss://lk.test".into(),
        api_key: "APIkey".into(),
        api_secret: secret.into(),
    }
}

async fn ctx() -> Option<Ctx> {
    ctx_with(lk_cfg(SECRET)).await
}

async fn ctx_with(cfg: LiveKitConfig) -> Option<Ctx> {
    let Ok(db) = std::env::var("DATABASE_URL") else {
        println!("DATABASE_URL unset; skipping");
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
    let dir = Arc::new(InMemoryRoomDirectory::default());
    let lk = Arc::new(InMemoryRoomService::default());
    let events = Arc::new(InMemoryMeetingEventPublisher::default());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let module = MeetingModule::new(MeetingDeps {
        pool: pool.clone(),
        jwt: keys.clone(),
        clock,
        livekit: cfg.clone(),
        rooms: dir.clone(),
        room_service: lk.clone(),
        events: events.clone(),
        token_ttl: DEFAULT_TOKEN_TTL,
    });
    Some(Ctx {
        pool,
        router: module.router(),
        keys,
        dir,
        lk,
        events,
        cfg,
    })
}

impl Ctx {
    /// New translation room with a fresh host.
    fn room(&self, status: &str) -> (RoomId, UserId) {
        let room = RoomId::new();
        let host = UserId::new();
        self.dir.set_room(
            room,
            RoomInfo {
                workspace_id: WorkspaceId::new(),
                host_id: host,
                status: status.into(),
            },
        );
        (room, host)
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn post(&self, uri: &str, user: UserId) -> (StatusCode, Value) {
        let token = self
            .keys
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
            .unwrap()
            .0;
        self.send(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn webhook(&self, body: &Value, secret: &str) -> StatusCode {
        let raw = body.to_string();
        let auth = sign_webhook(raw.as_bytes(), &lk_cfg(secret)).unwrap();
        self.send(
            Request::builder()
                .method(Method::POST)
                .uri("/meetings/webhooks/livekit")
                .header("authorization", auth)
                .header("content-type", "application/webhook+json")
                .body(Body::from(raw))
                .unwrap(),
        )
        .await
        .0
    }

    async fn participant(&self, room: RoomId, identity: &str) -> (bool, Option<i64>, Option<i64>) {
        let r = sqlx::query(
            "SELECT p.is_active, \
                    EXTRACT(EPOCH FROM p.joined_at)::bigint AS j, \
                    EXTRACT(EPOCH FROM p.left_at)::bigint AS l \
             FROM meeting.rtc_stream_participants p \
             JOIN meeting.meeting_rooms m ON m.id = p.meeting_room_id \
             WHERE m.translation_room_id = $1 AND p.provider_identity = $2",
        )
        .bind(room.as_uuid())
        .bind(identity)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        (
            r.get("is_active"),
            r.get::<Option<i64>, _>("j"),
            r.get::<Option<i64>, _>("l"),
        )
    }
}

fn decode(token: &str) -> TokenClaims {
    let mut v = Validation::new(Algorithm::HS256);
    v.set_issuer(&["APIkey"]);
    v.required_spec_claims.clear();
    jsonwebtoken::decode::<TokenClaims>(token, &DecodingKey::from_secret(SECRET.as_bytes()), &v)
        .unwrap()
        .claims
}

fn event(kind: &str, room: RoomId, identity: &str, at: i64) -> Value {
    json!({
        "event": kind,
        "id": format!("EV_{}", Uuid::new_v4()),
        "room": { "name": room.to_string(), "sid": "RM_x" },
        "participant": { "identity": identity, "sid": "PA_x" },
        "createdAt": at,
    })
}

#[tokio::test]
async fn join_gate() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let guest = UserId::new();
    let stranger = UserId::new();
    let uri = format!("/meetings/{room}/join");

    c.dir.set_participant(room, guest, "WAITING", "Guest");
    let (st, body) = c.post(&uri, guest).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "IN_LOBBY");
    assert!(
        c.lk.calls().is_empty(),
        "no livekit room for a lobby caller"
    );

    let (st, body) = c.post(&uri, stranger).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "FORBIDDEN");

    c.dir.set_participant(room, guest, "CONNECTED", "Guest");
    let (st, body) = c.post(&uri, guest).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["roomName"], room.to_string());
    assert_eq!(body["serverUrl"], "wss://lk.test");
    assert_eq!(body["participantIdentity"], guest.to_string());
    let claims = decode(body["token"].as_str().unwrap());
    assert_eq!(claims.sub, guest.to_string());
    assert_eq!(claims.name.as_deref(), Some("Guest"));
    assert_eq!(
        claims.video.room.as_deref(),
        Some(room.to_string().as_str())
    );
    assert!(claims.video.room_join && claims.video.can_publish && claims.video.can_subscribe);
    assert_eq!(claims.exp - claims.nbf, 4 * 3600);
    assert_eq!(
        c.lk.calls(),
        vec![RoomServiceCall::CreateRoom(room.to_string())]
    );

    // Host joins without a participant row; second join does not create the room again.
    let (st, _) = c.post(&uri, host).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(c.lk.calls().len(), 1);
    let row = sqlx::query(
        "SELECT status, active_host_id FROM meeting.meeting_rooms WHERE translation_room_id = $1",
    )
    .bind(room.as_uuid())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "ACTIVE");
    assert_eq!(
        row.get::<Option<Uuid>, _>("active_host_id"),
        Some(host.as_uuid())
    );

    // Unknown and ended rooms.
    let (st, _) = c
        .post(&format!("/meetings/{}/join", RoomId::new()), host)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (ended, ended_host) = c.room("ENDED");
    let (st, body) = c.post(&format!("/meetings/{ended}/join"), ended_host).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "INVALID_STATE");
}

fn bridge(provider: &str, capturer: Option<UserId>, stale: bool) -> BridgeInfo {
    BridgeInfo {
        provider: provider.into(),
        capturer_user_id: capturer,
        capturer_stale: stale,
    }
}

#[tokio::test]
async fn bridge_token_refuses_native_rooms() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    assert_eq!(
        c.post(&format!("/meetings/{room}/join"), host).await.0,
        StatusCode::OK
    );
    let (st, body) = c
        .post(&format!("/meetings/{room}/bridge-token"), host)
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "NOT_BRIDGE_ROOM");
}

#[tokio::test]
async fn bridge_token_is_capturer_only_and_publish_only() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let (capturer, other) = (UserId::new(), UserId::new());
    c.dir
        .set_bridge(room, bridge("google_meet", Some(capturer), false));
    assert_eq!(
        c.post(&format!("/meetings/{room}/join"), host).await.0,
        StatusCode::OK
    );
    let uri = format!("/meetings/{room}/bridge-token");

    // neither a stranger nor the host may mint it while someone else is the capturer
    for u in [other, host] {
        let (st, body) = c.post(&uri, u).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["code"], "NOT_CAPTURER");
    }
    let (st, body) = c.post(&uri, capturer).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["participantIdentity"],
        "00000000-0000-0000-0000-00000000b21d"
    );
    let claims = decode(body["token"].as_str().unwrap());
    assert_eq!(claims.sub, "00000000-0000-0000-0000-00000000b21d");
    assert!(claims.video.can_publish);
    assert!(!claims.video.can_subscribe);
    let meta: Value = serde_json::from_str(claims.metadata.as_deref().unwrap()).unwrap();
    assert_eq!(meta["provider"], "google_meet");

    // a stale capturer loses the right
    c.dir
        .set_bridge(room, bridge("google_meet", Some(capturer), true));
    let (st, body) = c.post(&uri, capturer).await;
    assert_eq!(
        (st, body["code"].as_str().unwrap()),
        (StatusCode::FORBIDDEN, "NOT_CAPTURER")
    );
}

#[tokio::test]
async fn bridge_token_falls_back_to_host_without_capturer() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    c.dir.set_bridge(room, bridge("zoom", None, false));
    assert_eq!(
        c.post(&format!("/meetings/{room}/join"), host).await.0,
        StatusCode::OK
    );
    let uri = format!("/meetings/{room}/bridge-token");
    assert_eq!(c.post(&uri, UserId::new()).await.0, StatusCode::FORBIDDEN);
    let (st, body) = c.post(&uri, host).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn end_requires_host_and_publishes_once() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let guest = UserId::new();
    c.dir.set_participant(room, guest, "CONNECTED", "G");
    let join = format!("/meetings/{room}/join");
    let end = format!("/meetings/{room}/end");

    assert_eq!(c.post(&join, guest).await.0, StatusCode::OK);
    let (st, body) = c.post(&end, guest).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert!(c.events.events().is_empty());

    assert_eq!(c.post(&end, host).await.0, StatusCode::NO_CONTENT);
    let types: Vec<_> = c.events.events().iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EVENT_END_ROOM, EVENT_BILLING_STOP]);
    assert!(
        c.lk.calls()
            .contains(&RoomServiceCall::DeleteRoom(room.to_string()))
    );
    let row = sqlx::query(
        "SELECT status, ended_at IS NOT NULL AS ended FROM meeting.meeting_rooms \
         WHERE translation_room_id = $1",
    )
    .bind(room.as_uuid())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "FINISHED");
    assert!(row.get::<bool, _>("ended"));

    // Repeat End is a no-op; joining the finished session is refused.
    assert_eq!(c.post(&end, host).await.0, StatusCode::NO_CONTENT);
    assert_eq!(c.events.events().len(), 2);
    assert_eq!(c.post(&join, guest).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn webhook_updates_roster_and_replay_is_idempotent() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let guest = UserId::new();
    c.dir.set_participant(room, guest, "CONNECTED", "G");
    let join = format!("/meetings/{room}/join");
    assert_eq!(c.post(&join, host).await.0, StatusCode::OK);
    assert_eq!(c.post(&join, guest).await.0, StatusCode::OK);
    let gid = guest.to_string();

    let t0 = Utc::now().timestamp() - 100;
    let joined = event("participant_joined", room, &gid, t0);
    assert_eq!(c.webhook(&joined, SECRET).await, StatusCode::OK);
    assert_eq!(c.participant(room, &gid).await, (true, Some(t0), None));

    let left = event("participant_left", room, &gid, t0 + 50);
    assert_eq!(c.webhook(&left, SECRET).await, StatusCode::OK);
    assert_eq!(
        c.participant(room, &gid).await,
        (false, Some(t0), Some(t0 + 50))
    );

    // Redelivery, in any order, changes nothing.
    for ev in [&left, &joined, &left, &joined] {
        assert_eq!(c.webhook(ev, SECRET).await, StatusCode::OK);
    }
    assert_eq!(
        c.participant(room, &gid).await,
        (false, Some(t0), Some(t0 + 50))
    );

    // A later rejoin does apply.
    let rejoin = event("participant_joined", room, &gid, t0 + 80);
    assert_eq!(c.webhook(&rejoin, SECRET).await, StatusCode::OK);
    assert_eq!(c.participant(room, &gid).await, (true, Some(t0 + 80), None));

    // Tracks: upsert by track sid, replay safe.
    let sid = format!("TR_{}", Uuid::new_v4().simple());
    let mut tp = event("track_published", room, &gid, t0 + 90);
    tp["track"] = json!({ "sid": sid, "type": "AUDIO" });
    let mut tm = event("track_muted", room, &gid, t0 + 95);
    tm["track"] = json!({ "sid": sid, "type": "AUDIO" });
    for ev in [&tp, &tm, &tp, &tm] {
        assert_eq!(c.webhook(ev, SECRET).await, StatusCode::OK);
    }
    let r = sqlx::query(
        "SELECT count(*) OVER () AS n, media_type, is_muted, is_active \
         FROM meeting.meeting_tracks WHERE provider_track_id = $1",
    )
    .bind(&sid)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(r.get::<i64, _>("n"), 1);
    assert_eq!(r.get::<String, _>("media_type"), "AUDIO");
    assert!(r.get::<bool, _>("is_muted"));
    assert!(r.get::<bool, _>("is_active"));

    // room_finished closes the session and everyone in it; replay is harmless.
    let fin = event("room_finished", room, "", t0 + 200);
    for _ in 0..2 {
        assert_eq!(c.webhook(&fin, SECRET).await, StatusCode::OK);
    }
    assert!(!c.participant(room, &gid).await.0);
    let r =
        sqlx::query("SELECT is_active FROM meeting.meeting_rooms WHERE translation_room_id = $1")
            .bind(room.as_uuid())
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert!(!r.get::<bool, _>("is_active"));

    // Events for rooms we do not know are acknowledged.
    let unknown = event("participant_joined", RoomId::new(), "x", t0);
    assert_eq!(c.webhook(&unknown, SECRET).await, StatusCode::OK);
}

#[tokio::test]
async fn webhook_rejects_bad_signature() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    assert_eq!(
        c.post(&format!("/meetings/{room}/join"), host).await.0,
        StatusCode::OK
    );
    let ev = event(
        "participant_left",
        room,
        &host.to_string(),
        Utc::now().timestamp(),
    );

    // Wrong secret.
    assert_eq!(
        c.webhook(&ev, "another-secret-another-secret-0002").await,
        StatusCode::UNAUTHORIZED
    );
    // Tampered body.
    let auth = sign_webhook(ev.to_string().as_bytes(), &c.cfg).unwrap();
    let mut tampered = ev.clone();
    tampered["participant"]["identity"] = json!("someone-else");
    let (st, body) = c
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/meetings/webhooks/livekit")
                .header("authorization", auth)
                .body(Body::from(tampered.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "UNAUTHORIZED");
    // Missing header.
    let (st, _) = c
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/meetings/webhooks/livekit")
                .body(Body::from(ev.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // Nothing was applied.
    let (active, _, left) = c.participant(room, &host.to_string()).await;
    assert!(active);
    assert_eq!(left, None);
}

#[tokio::test]
async fn join_requires_auth() {
    let Some(c) = ctx().await else { return };
    let (room, _) = c.room("IN_PROGRESS");
    let (st, _) = c
        .send(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/meetings/{room}/join"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn end_without_meeting_row_ends_room_through_directory() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let guest = UserId::new();
    c.dir.set_participant(room, guest, "CONNECTED", "G");
    let end = format!("/meetings/{room}/end");

    // Only the host may end, even before any session exists.
    assert_eq!(c.post(&end, guest).await.0, StatusCode::FORBIDDEN);
    assert!(c.dir.ended_rooms().is_empty());

    assert_eq!(c.post(&end, host).await.0, StatusCode::NO_CONTENT);
    assert_eq!(c.dir.ended_rooms(), vec![(room, host)]);
    let types: Vec<_> = c.events.events().iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EVENT_END_ROOM, EVENT_BILLING_STOP]);
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM meeting.meeting_rooms WHERE translation_room_id = $1")
            .bind(room.as_uuid())
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert_eq!(n, 0);

    // Repeat End: room already ended, nothing more happens.
    assert_eq!(c.post(&end, host).await.0, StatusCode::NO_CONTENT);
    assert_eq!(c.dir.ended_rooms().len(), 1);
    assert_eq!(c.events.events().len(), 2);
}

#[tokio::test]
async fn end_works_when_livekit_is_not_configured() {
    let Some(c) = ctx_with(LiveKitConfig {
        url: String::new(),
        api_key: String::new(),
        api_secret: String::new(),
    })
    .await
    else {
        return;
    };
    // No session yet.
    let (room, host) = c.room("WAITING");
    let end = format!("/meetings/{room}/end");
    assert_eq!(c.post(&end, host).await.0, StatusCode::NO_CONTENT);
    assert_eq!(c.dir.ended_rooms(), vec![(room, host)]);
    assert_eq!(c.events.events().len(), 2);
    assert!(c.lk.calls().is_empty());

    // A session row exists (e.g. created while LiveKit was configured): the room is still ended.
    let (room2, host2) = c.room("IN_PROGRESS");
    sqlx::query(
        "INSERT INTO meeting.meeting_rooms (id, translation_room_id, provider_room_name, status, \
         active_host_id, is_current, created_at, updated_at) \
         VALUES ($1, $2, $3, 'ACTIVE', $4, true, now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(room2.as_uuid())
    .bind(room2.to_string())
    .bind(host2.as_uuid())
    .execute(&c.pool)
    .await
    .unwrap();
    let end2 = format!("/meetings/{room2}/end");
    assert_eq!(c.post(&end2, host2).await.0, StatusCode::NO_CONTENT);
    assert_eq!(c.dir.ended_rooms().len(), 2);
    assert_eq!(c.events.events().len(), 4);
}

#[tokio::test]
async fn invalid_json_body_is_a_json_error() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let token = c
        .keys
        .issue_access(host, Uuid::new_v4(), false, Utc::now())
        .unwrap()
        .0;
    let (st, v) = c
        .send(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/meetings/{room}/join"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "INVALID_REQUEST", "{v}");
    assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));

    let (st, v) = c.post("/meetings/not-a-uuid/end", host).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "INVALID_REQUEST", "{v}");
}

// ---------------------------------------------------------------- chat

impl Ctx {
    async fn call(
        &self,
        m: Method,
        uri: &str,
        user: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let token = self
            .keys
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
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
        self.send(b.body(body).unwrap()).await
    }

    async fn say(&self, room: RoomId, user: UserId, text: &str, cid: &str) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            &format!("/meetings/{room}/chat/messages"),
            user,
            Some(json!({"content": text, "clientMessageId": cid})),
        )
        .await
    }

    async fn history(&self, room: RoomId, user: UserId, query: &str) -> (StatusCode, Value) {
        self.call(
            Method::GET,
            &format!("/meetings/{room}/chat/messages{query}"),
            user,
            None,
        )
        .await
    }

    /// Room in progress, a joined host and one CONNECTED guest who joined too.
    async fn chat_room(&self) -> (RoomId, UserId, UserId) {
        let (room, host) = self.room("IN_PROGRESS");
        let guest = UserId::new();
        self.dir.set_participant(room, guest, "CONNECTED", "Guest");
        for u in [host, guest] {
            let (st, b) = self.post(&format!("/meetings/{room}/join"), u).await;
            assert_eq!(st, StatusCode::OK, "{b}");
        }
        (room, host, guest)
    }
}

#[tokio::test]
async fn chat_post_history_and_idempotency() {
    let Some(c) = ctx().await else { return };
    let (room, host, guest) = c.chat_room().await;

    let (st, m1) = c.say(room, guest, "  hello  ", "c-1").await;
    assert_eq!(st, StatusCode::CREATED, "{m1}");
    assert_eq!(m1["content"], "hello");
    assert_eq!(m1["senderId"], guest.to_string());
    assert_eq!(m1["senderName"], "Guest");
    assert_eq!(m1["hidden"], false);
    assert_eq!(m1["translations"], json!({}));
    assert!(m1["messageId"].is_string() && m1["createdAt"].is_string());

    // replay with the same clientMessageId: same message, no duplicate, no second publish
    let (st, again) = c.say(room, guest, "hello", "c-1").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(again["messageId"], m1["messageId"]);
    // same id from another sender is a different message
    let (st, m2) = c.say(room, host, "hi guest", "c-1").await;
    assert_eq!(st, StatusCode::CREATED, "{m2}");
    assert_ne!(m2["messageId"], m1["messageId"]);

    let requests: Vec<_> = c
        .events
        .events()
        .into_iter()
        .filter(|e| e.event_type == EVENT_CHAT_TRANSLATE)
        .filter(|e| e.payload["translation_room_id"] == room.to_string())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].payload["text"], "hello");
    assert_eq!(requests[0].payload["message_id"], m1["messageId"]);

    // history: newest last
    let (st, h) = c.history(room, host, "").await;
    assert_eq!(st, StatusCode::OK, "{h}");
    let ids: Vec<_> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["messageId"].clone())
        .collect();
    assert_eq!(ids, vec![m1["messageId"].clone(), m2["messageId"].clone()]);

    // paging with limit + before
    for i in 0..4 {
        c.say(room, guest, &format!("m{i}"), &format!("p-{i}"))
            .await;
    }
    let (_, page) = c.history(room, guest, "?limit=3").await;
    let page = page.as_array().unwrap().clone();
    assert_eq!(page.len(), 3);
    assert_eq!(
        page.iter()
            .map(|m| m["content"].clone())
            .collect::<Vec<_>>(),
        vec![json!("m1"), json!("m2"), json!("m3")]
    );
    let before = page[0]["messageId"].as_str().unwrap();
    let (_, older) = c
        .history(room, guest, &format!("?limit=50&before={before}"))
        .await;
    assert_eq!(
        older
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].clone())
            .collect::<Vec<_>>(),
        vec![json!("hello"), json!("hi guest"), json!("m0")]
    );
    // limit is clamped to 1..50 instead of failing
    assert_eq!(
        c.history(room, guest, "?limit=0")
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(c.history(room, guest, "?limit=500").await.0, StatusCode::OK);

    // translations written by the worker are returned
    sqlx::query(
        "INSERT INTO meeting.meeting_chat_translations \
         (id, message_id, meeting_room_id, source_language, target_language, translated_text, created_at) \
         SELECT $1, id, meeting_room_id, 'en', 'vi', 'xin chao', now() \
         FROM meeting.meeting_chat_messages WHERE id = $2",
    )
    .bind(Uuid::now_v7())
    .bind(m1["messageId"].as_str().unwrap().parse::<Uuid>().unwrap())
    .execute(&c.pool)
    .await
    .unwrap();
    let (_, h) = c
        .history(
            room,
            host,
            "?limit=50&before=00000000-0000-0000-0000-000000000000",
        )
        .await;
    assert_eq!(h, json!([]));
    let (_, h) = c.history(room, host, "").await;
    assert_eq!(h[0]["translations"], json!({"vi": "xin chao"}));
}

#[tokio::test]
async fn chat_validation_and_access() {
    let Some(c) = ctx().await else { return };
    let (room, host, guest) = c.chat_room().await;

    // content bounds: 1000 chars ok (multi-byte counted as chars), 1001 rejected, blank rejected
    let ok = "é".repeat(1000);
    assert_eq!(
        c.say(room, guest, &ok, "len-ok").await.0,
        StatusCode::CREATED
    );
    let (st, b) = c.say(room, guest, &"é".repeat(1001), "len-bad").await;
    assert_eq!(
        (st, b["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("VALIDATION_ERROR"))
    );
    assert_eq!(
        c.say(room, guest, "   ", "blank").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(c.say(room, guest, "x", "").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        c.say(room, guest, "x", &"i".repeat(101)).await.0,
        StatusCode::BAD_REQUEST
    );

    // non-participant / waiting / kicked cannot post or read
    let stranger = UserId::new();
    let waiting = UserId::new();
    let kicked = UserId::new();
    c.dir.set_participant(room, waiting, "WAITING", "W");
    c.dir.set_participant(room, kicked, "KICKED", "K");
    for u in [stranger, waiting, kicked] {
        assert_eq!(c.say(room, u, "hey", "x-1").await.0, StatusCode::FORBIDDEN);
        assert_eq!(c.history(room, u, "").await.0, StatusCode::FORBIDDEN);
    }
    // unknown room
    assert_eq!(
        c.say(RoomId::new(), host, "hey", "x").await.0,
        StatusCode::NOT_FOUND
    );

    // a participant who left keeps read access but cannot post
    let leaver = UserId::new();
    c.dir.set_participant(room, leaver, "LEFT", "L");
    assert_eq!(c.history(room, leaver, "").await.0, StatusCode::OK);
    assert_eq!(
        c.say(room, leaver, "bye", "l-1").await.0,
        StatusCode::FORBIDDEN
    );

    // ended meeting: no new messages, history still readable
    let (st, _) = c.post(&format!("/meetings/{room}/end"), host).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, b) = c.say(room, guest, "late", "late-1").await;
    assert_eq!(
        (st, b["code"].as_str()),
        (StatusCode::CONFLICT, Some("INVALID_STATE"))
    );
    assert_eq!(c.history(room, guest, "").await.0, StatusCode::OK);
}

#[tokio::test]
async fn chat_requires_a_started_meeting() {
    let Some(c) = ctx().await else { return };
    let (room, host) = c.room("IN_PROGRESS");
    let (st, _) = c.say(room, host, "anyone?", "n-1").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, h) = c.history(room, host, "").await;
    assert_eq!((st, h), (StatusCode::OK, json!([])));
}

#[tokio::test]
async fn chat_hide_is_host_only_and_not_destructive() {
    let Some(c) = ctx().await else { return };
    let (room, host, guest) = c.chat_room().await;
    let other = UserId::new();
    c.dir.set_participant(room, other, "CONNECTED", "Other");
    let (_, m) = c.say(room, guest, "rude words", "h-1").await;
    let (_, keep) = c.say(room, guest, "fine", "h-2").await;
    let id = m["messageId"].as_str().unwrap();
    let uri = format!("/meetings/{room}/chat/messages/{id}/hide");

    // non-host (even the author) is refused
    for u in [guest, other] {
        let (st, b) = c.call(Method::POST, &uri, u, None).await;
        assert_eq!(
            (st, b["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("FORBIDDEN"))
        );
    }
    assert_eq!(
        c.history(room, guest, "").await.1.as_array().unwrap().len(),
        2
    );

    let (st, b) = c
        .call(Method::POST, &uri, host, Some(json!({"reason": "abuse"})))
        .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(
        (b["messageId"].as_str(), b["hidden"].as_bool()),
        (Some(id), Some(true))
    );
    // idempotent, one audit row
    assert_eq!(
        c.call(Method::POST, &uri, host, None).await.0,
        StatusCode::OK
    );

    // others no longer see it, the host still does (flagged)
    let (_, h) = c.history(room, guest, "").await;
    assert_eq!(h.as_array().unwrap().len(), 1);
    assert_eq!(h[0]["messageId"], keep["messageId"]);
    let (_, h) = c.history(room, host, "").await;
    assert_eq!(h.as_array().unwrap().len(), 2);
    assert_eq!(
        (h[0]["content"].as_str(), h[0]["hidden"].as_bool()),
        (Some("rude words"), Some(true))
    );
    assert_eq!(h[1]["hidden"], false);

    // BR-54: the row and one moderation event remain
    let (rows, events): (i64, i64) = (
        sqlx::query_scalar("SELECT count(*) FROM meeting.meeting_chat_messages WHERE id = $1 AND is_hidden")
            .bind(id.parse::<Uuid>().unwrap())
            .fetch_one(&c.pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM meeting.meeting_chat_moderation_events WHERE message_id = $1 AND action = 'hidden'")
            .bind(id.parse::<Uuid>().unwrap())
            .fetch_one(&c.pool)
            .await
            .unwrap(),
    );
    assert_eq!((rows, events), (1, 1));
    assert!(
        c.events
            .events()
            .iter()
            .any(|e| e.event_type == EVENT_CHAT_HIDDEN && e.payload["message_id"] == id)
    );

    // unknown message / message of another meeting: 404
    let missing = format!("/meetings/{room}/chat/messages/{}/hide", Uuid::now_v7());
    assert_eq!(
        c.call(Method::POST, &missing, host, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (room2, host2, _) = c.chat_room().await;
    let cross = format!("/meetings/{room2}/chat/messages/{id}/hide");
    assert_eq!(
        c.call(Method::POST, &cross, host2, None).await.0,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------- chat translation consumer

mod translate {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use warptalk_bus::{HandlerError, StreamHandler, StreamMessage};
    use warptalk_contracts::events::DomainEventEnvelope;
    use warptalk_kernel::AppResult;
    use warptalk_meeting::application::ports::{
        ChatEventSink, ChatTargetLanguages, ChatTranslation, ChatTranslator, TranslateError,
    };
    use warptalk_meeting::infrastructure::ChatTranslateHandler;

    struct Targets(Vec<String>);
    #[async_trait::async_trait]
    impl ChatTargetLanguages for Targets {
        async fn for_room(&self, _: RoomId) -> AppResult<Vec<String>> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct FakeTranslator {
        calls: Mutex<Vec<(String, String)>>,
        fail_transient: Mutex<bool>,
    }
    #[async_trait::async_trait]
    impl ChatTranslator for FakeTranslator {
        async fn translate(
            &self,
            _: WorkspaceId,
            text: &str,
            source: &str,
            target: &str,
        ) -> Result<ChatTranslation, TranslateError> {
            self.calls
                .lock()
                .unwrap()
                .push((source.to_owned(), target.to_owned()));
            if *self.fail_transient.lock().unwrap() {
                return Err(TranslateError::Transient("overloaded".into()));
            }
            Ok(ChatTranslation {
                text: format!("[{target}] {text}"),
                model: Some("fake".into()),
            })
        }
    }

    #[derive(Default)]
    struct Sink(Mutex<Vec<(String, Value)>>);
    #[async_trait::async_trait]
    impl ChatEventSink for Sink {
        async fn publish(&self, channel: &str, payload: Value) -> AppResult<()> {
            self.0.lock().unwrap().push((channel.to_owned(), payload));
            Ok(())
        }
    }

    fn stream_msg(env: &DomainEventEnvelope) -> StreamMessage {
        StreamMessage {
            stream: "translate:chat_requests".into(),
            id: "1-0".into(),
            fields: HashMap::from([("data".to_owned(), serde_json::to_string(env).unwrap())]),
        }
    }

    /// Post `text` as the host and return the request envelope the service published.
    async fn post(
        c: &Ctx,
        room: RoomId,
        host: UserId,
        text: &str,
        cid: &str,
    ) -> DomainEventEnvelope {
        let (st, m) = c.say(room, host, text, cid).await;
        assert_eq!(st, StatusCode::CREATED, "{m}");
        let ev = c
            .events
            .events()
            .into_iter()
            .rev()
            .find(|e| {
                e.event_type == EVENT_CHAT_TRANSLATE && e.payload["message_id"] == m["messageId"]
            })
            .unwrap();
        DomainEventEnvelope {
            event_id: Uuid::now_v7(),
            event_type: ev.event_type.to_owned(),
            schema_version: 1,
            producer: "meeting-service".into(),
            occurred_at: Utc::now(),
            payload: ev.payload,
        }
    }

    #[tokio::test]
    async fn translates_stores_idempotently_and_skips_same_language() {
        let Some(c) = ctx().await else { return };
        let (room, host, _guest) = c.chat_room().await;
        let translator = Arc::new(FakeTranslator::default());
        let sink = Arc::new(Sink::default());
        let handler = ChatTranslateHandler::new(
            c.pool.clone(),
            Arc::new(Targets(vec!["en".into(), "vi-VN".into(), "ja".into()])),
            translator.clone(),
            sink.clone(),
            Arc::new(SystemClock),
        );

        // Vietnamese text: source vi (heuristic) -> only en + ja are translated.
        let env = post(&c, room, host, "Xin chào mọi người", "t-1").await;
        handler.handle(&stream_msg(&env)).await.unwrap();
        let langs: Vec<String> = translator
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(s, t)| {
                assert_eq!(s, "vi");
                t.clone()
            })
            .collect();
        assert_eq!(langs, vec!["en", "ja"]);

        let mid = env.payload["message_id"].as_str().unwrap().to_owned();
        let row = sqlx::query(
            "SELECT original_language, (SELECT count(*) FROM meeting.meeting_chat_translations \
             WHERE message_id = $1::uuid) AS n FROM meeting.meeting_chat_messages WHERE id = $1::uuid",
        )
        .bind(&mid)
        .fetch_one(&c.pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("original_language"), "vi");
        assert_eq!(row.get::<i64, _>("n"), 2);

        let published = sink.0.lock().unwrap().clone();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, format!("meeting:{room}:chat"));
        assert_eq!(published[0].1["type"], "chat.translated");
        assert_eq!(published[0].1["messageId"], mid);
        assert_eq!(
            published[0].1["translations"]["en"],
            "[en] Xin chào mọi người"
        );

        // redelivery: nothing translated again, nothing stored twice, nothing republished
        handler.handle(&stream_msg(&env)).await.unwrap();
        assert_eq!(translator.calls.lock().unwrap().len(), 2);
        assert_eq!(sink.0.lock().unwrap().len(), 1);

        // history carries the translations map
        let (st, h) = c.history(room, host, "").await;
        assert_eq!(st, StatusCode::OK, "{h}");
        let m = &h.as_array().unwrap()[0];
        assert_eq!(m["translations"]["ja"], "[ja] Xin chào mọi người");
        assert_eq!(m["translations"].as_object().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn transient_failure_retries_and_garbage_is_fatal() {
        let Some(c) = ctx().await else { return };
        let (room, host, _guest) = c.chat_room().await;
        let translator = Arc::new(FakeTranslator::default());
        let sink = Arc::new(Sink::default());
        let handler = ChatTranslateHandler::new(
            c.pool.clone(),
            Arc::new(Targets(vec!["en".into(), "vi".into()])),
            translator.clone(),
            sink.clone(),
            Arc::new(SystemClock),
        );
        let env = post(&c, room, host, "hello", "t-2").await;

        *translator.fail_transient.lock().unwrap() = true;
        let r = handler.handle(&stream_msg(&env)).await;
        assert!(matches!(r, Err(HandlerError::Retry(_))), "{r:?}");
        assert!(sink.0.lock().unwrap().is_empty());

        // provider recovers: the redelivery completes the work
        *translator.fail_transient.lock().unwrap() = false;
        handler.handle(&stream_msg(&env)).await.unwrap();
        assert_eq!(sink.0.lock().unwrap().len(), 1);
        // source en: only vi was requested (en skipped as same language)
        assert!(
            translator
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|(_, t)| t == "vi")
        );

        let bad = StreamMessage {
            stream: "translate:chat_requests".into(),
            id: "2-0".into(),
            fields: HashMap::from([("data".to_owned(), "not json".to_owned())]),
        };
        assert!(matches!(
            handler.handle(&bad).await,
            Err(HandlerError::Fatal(_))
        ));
    }
}
