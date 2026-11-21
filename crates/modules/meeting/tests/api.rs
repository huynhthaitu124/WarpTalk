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
    BridgeInfo, EVENT_BILLING_STOP, EVENT_END_ROOM, InMemoryMeetingEventPublisher,
    InMemoryRoomDirectory, RoomInfo,
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
