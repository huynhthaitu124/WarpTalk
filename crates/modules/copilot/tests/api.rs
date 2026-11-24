#![allow(clippy::unwrap_used)]
//! Copilot router against the local Postgres with in-memory room/workspace/sink fakes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_copilot::application::ports::{
    CreatedRoom, MemorySegmentSink, NewSessionRoom, NoRateLimit, NoSttTokenIssuer, RateLimiter,
    RoomState, SessionRooms, SttIssueError, SttToken, SttTokenIssuer, SttTokenRequest,
    WorkspaceMembers,
};
use warptalk_copilot::{CopilotDeps, CopilotModule, MIGRATIONS};
use warptalk_kernel::{AppResult, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;

#[derive(Default)]
struct FakeRooms {
    rooms: Mutex<HashMap<RoomId, RoomState>>,
}

#[async_trait]
impl SessionRooms for FakeRooms {
    async fn create(&self, user: UserId, new: NewSessionRoom) -> AppResult<CreatedRoom> {
        let id = RoomId::new();
        let now = Utc::now();
        self.rooms.lock().unwrap().insert(
            id,
            RoomState {
                workspace_id: new.workspace_id,
                host_id: user,
                status: "IN_PROGRESS".into(),
                started_at: Some(now),
                ended_at: None,
            },
        );
        Ok(CreatedRoom {
            room_id: id,
            host_participant_id: Uuid::now_v7(),
            started_at: now,
        })
    }
    async fn state(&self, room: RoomId) -> AppResult<Option<RoomState>> {
        Ok(self.rooms.lock().unwrap().get(&room).cloned())
    }
    async fn end(&self, room: RoomId, _: UserId) -> AppResult<()> {
        if let Some(r) = self.rooms.lock().unwrap().get_mut(&room) {
            r.status = "ENDED".into();
            r.ended_at = Some(Utc::now());
        }
        Ok(())
    }
}

#[derive(Default)]
struct FakeMembers(Mutex<HashMap<(WorkspaceId, UserId), WorkspaceRole>>);

#[async_trait]
impl WorkspaceMembers for FakeMembers {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(self.0.lock().unwrap().get(&(ws, user)).copied())
    }
}

struct FakeIssuer;

#[async_trait]
impl SttTokenIssuer for FakeIssuer {
    async fn issue(&self, req: SttTokenRequest) -> Result<SttToken, SttIssueError> {
        Ok(SttToken {
            provider: "openai".into(),
            model: "m".into(),
            token: format!("ek_{}", req.session),
            expires_at: Utc::now(),
            websocket_url: "wss://x".into(),
            session_config: json!({"type": "transcription"}),
        })
    }
}

struct CountingLimiter(Mutex<u32>);

#[async_trait]
impl RateLimiter for CountingLimiter {
    async fn allow(&self, _: &str, limit: u32, _: u64) -> AppResult<bool> {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        Ok(*n <= limit)
    }
}

struct Ctx {
    app: Router,
    keys: JwtKeys,
    rooms: Arc<FakeRooms>,
    members: Arc<FakeMembers>,
    sink: Arc<MemorySegmentSink>,
    ws: WorkspaceId,
}

async fn ctx(issuer: Arc<dyn SttTokenIssuer>, limiter: Arc<dyn RateLimiter>) -> Option<Ctx> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
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
    let rooms = Arc::new(FakeRooms::default());
    let members = Arc::new(FakeMembers::default());
    let sink = Arc::new(MemorySegmentSink::default());
    let module = CopilotModule::new(CopilotDeps {
        pool,
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        rooms: rooms.clone(),
        members: members.clone(),
        sink: sink.clone(),
        stt_tokens: issuer,
        rate_limiter: limiter,
    });
    Some(Ctx {
        app: module.router(),
        keys,
        rooms,
        members,
        sink,
        ws: WorkspaceId::new(),
    })
}

impl Ctx {
    fn user(&self, role: Option<WorkspaceRole>) -> (UserId, String) {
        let id = UserId::new();
        if let Some(r) = role {
            self.members.0.lock().unwrap().insert((self.ws, id), r);
        }
        let tok = self
            .keys
            .issue_access(id, Uuid::new_v4(), false, Utc::now())
            .unwrap()
            .0;
        (id, tok)
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        tok: &str,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {tok}"));
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn new_session(&self, tok: &str) -> String {
        let (s, b) = self
            .call(
                "POST",
                "/copilot/sessions",
                Some(json!({
                    "workspaceId": self.ws.to_string(),
                    "title": "Weekly sync",
                    "platform": "google_meet",
                    "meetingUrl": "https://meet.google.com/abc-defg-hij",
                    "languages": {"me": "vi", "others": "en"}
                })),
                tok,
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        b["id"].as_str().unwrap().to_owned()
    }
}

fn seg(id: &str, channel: &str) -> Value {
    json!({"clientSegmentId": id, "channel": channel, "text": "hello there", "language": "en",
           "startMs": 100, "endMs": 900, "confidence": 0.9, "isFinal": true})
}

#[tokio::test]
async fn session_lifecycle_and_access() {
    let Some(c) = ctx(Arc::new(NoSttTokenIssuer), Arc::new(NoRateLimit)).await else {
        return;
    };
    let (_, host) = c.user(Some(WorkspaceRole::Member));
    let (_, other) = c.user(Some(WorkspaceRole::Member));
    let (_, admin) = c.user(Some(WorkspaceRole::Admin));
    let (_, outsider) = c.user(None);

    let id = c.new_session(&host).await;
    let (s, b) = c
        .call("GET", &format!("/copilot/sessions/{id}"), None, &host)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "IN_PROGRESS");
    assert_eq!(b["roomId"], id);
    assert_eq!(b["platform"], "google_meet");
    assert_eq!(b["languages"], json!({"me": "vi", "others": "en"}));
    assert_eq!(b["displayNames"], json!({"me": "Tôi", "others": "Others"}));

    // A second, newer session: the list is newest first.
    let id2 = c.new_session(&host).await;
    let ws = c.ws;
    let (s, b) = c
        .call(
            "GET",
            &format!("/copilot/sessions?workspaceId={ws}"),
            None,
            &host,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = b["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![id2.as_str(), id.as_str()]);
    assert_eq!(b["total"], 2);

    // Members only see their own; admins see the workspace; outsiders are refused.
    let (_, b) = c
        .call(
            "GET",
            &format!("/copilot/sessions?workspaceId={ws}"),
            None,
            &other,
        )
        .await;
    assert_eq!(b["total"], 0);
    let (_, b) = c
        .call(
            "GET",
            &format!("/copilot/sessions?workspaceId={ws}"),
            None,
            &admin,
        )
        .await;
    assert_eq!(b["total"], 2);
    let (s, _) = c
        .call(
            "GET",
            &format!("/copilot/sessions?workspaceId={ws}"),
            None,
            &outsider,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = c
        .call("GET", &format!("/copilot/sessions/{id}"), None, &other)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = c
        .call("GET", &format!("/copilot/sessions/{id}"), None, &admin)
        .await;
    assert_eq!(s, StatusCode::OK);

    // End: idempotent.
    let (s, b) = c
        .call("POST", &format!("/copilot/sessions/{id}/end"), None, &host)
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "ENDED");
    assert!(b["endedAt"].is_string());
    let (s, b) = c
        .call("POST", &format!("/copilot/sessions/{id}/end"), None, &host)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "ENDED");
    assert_eq!(
        c.rooms
            .rooms
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.status == "ENDED")
            .count(),
        1
    );
}

#[tokio::test]
async fn create_validation() {
    let Some(c) = ctx(Arc::new(NoSttTokenIssuer), Arc::new(NoRateLimit)).await else {
        return;
    };
    let (_, host) = c.user(Some(WorkspaceRole::Member));
    let base = |patch: Value| {
        let mut v = json!({"workspaceId": c.ws.to_string(), "platform": "zoom",
                           "languages": {"me": "vi", "others": "en"}});
        for (k, val) in patch.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    };
    for (patch, code) in [
        (json!({"platform": "skype"}), "INVALID_PLATFORM"),
        (
            json!({"languages": {"me": "", "others": "en"}}),
            "INVALID_LANGUAGE",
        ),
        (
            json!({"meetingUrl": "javascript:alert(1)"}),
            "INVALID_MEETING_URL",
        ),
        (json!({"title": "x".repeat(256)}), "INVALID_TITLE"),
    ] {
        let (s, b) = c
            .call("POST", "/copilot/sessions", Some(base(patch)), &host)
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        assert_eq!(b["code"], code);
    }
    let (s, b) = c
        .call(
            "POST",
            "/copilot/sessions",
            Some(base(json!({"platform": "in_person"}))),
            &host,
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["title"], "Copilot session");
    let (s, _) = c
        .call(
            "POST",
            "/copilot/sessions",
            Some(base(json!({}))),
            "garbage",
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn segment_ingest_rules() {
    let Some(c) = ctx(Arc::new(NoSttTokenIssuer), Arc::new(NoRateLimit)).await else {
        return;
    };
    let (_, host) = c.user(Some(WorkspaceRole::Member));
    let (_, other) = c.user(Some(WorkspaceRole::Member));
    let (_, outsider) = c.user(None);
    let id = c.new_session(&host).await;
    let uri = format!("/copilot/sessions/{id}/segments");

    let (s, b) = c
        .call(
            "POST",
            &uri,
            Some(json!({"segments": [seg("a", "me"), seg("b", "others")]})),
            &host,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["accepted"].as_u64(), b["duplicates"].as_u64()),
        (Some(2), Some(0))
    );
    {
        let r = c.sink.results.lock().unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].far_speaker_name.as_deref(), Some("Tôi"));
        assert_eq!(r[1].far_speaker_name.as_deref(), Some("Others"));
        assert_ne!(r[0].speaker_id, r[1].speaker_id);
        assert_eq!(
            r[0].segment_id,
            b["segments"][0]["segmentId"].as_str().unwrap()
        );
        assert!(r[0].anchor_ms.is_some());
    }

    // Idempotent per clientSegmentId.
    let (_, b) = c
        .call(
            "POST",
            &uri,
            Some(json!({"segments": [seg("a", "me"), seg("c", "me")]})),
            &host,
        )
        .await;
    assert_eq!(
        (b["accepted"].as_u64(), b["duplicates"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(b["segments"][0]["duplicate"], true);
    assert_eq!(c.sink.results.lock().unwrap().len(), 3);

    // Validation: nothing is written when any segment is bad.
    let before = c.sink.results.lock().unwrap().len();
    let mut long = seg("d", "me");
    long["text"] = json!("x".repeat(2001));
    for bad in [
        json!({"segments": []}),
        json!({"segments": (0..51).map(|i| seg(&format!("m{i}"), "me")).collect::<Vec<_>>()}),
        json!({"segments": [seg("e", "me"), long]}),
        json!({"segments": [seg("f", "nobody")]}),
        json!({"segments": [{"clientSegmentId": "g", "channel": "me", "text": "t", "language": "en", "startMs": 9, "endMs": 1, "isFinal": true}]}),
    ] {
        let (s, b) = c.call("POST", &uri, Some(bad), &host).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    }
    assert_eq!(c.sink.results.lock().unwrap().len(), before);
    let fifty: Vec<Value> = (0..50).map(|i| seg(&format!("n{i}"), "me")).collect();
    let (s, _) = c
        .call("POST", &uri, Some(json!({"segments": fifty})), &host)
        .await;
    assert_eq!(s, StatusCode::OK);

    // Host only; outsiders cannot even see the session.
    let (s, _) = c
        .call(
            "POST",
            &uri,
            Some(json!({"segments": [seg("h", "me")]})),
            &other,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = c
        .call(
            "POST",
            &uri,
            Some(json!({"segments": [seg("h", "me")]})),
            &outsider,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Not after the end.
    c.call("POST", &format!("/copilot/sessions/{id}/end"), None, &host)
        .await;
    let (s, b) = c
        .call(
            "POST",
            &uri,
            Some(json!({"segments": [seg("i", "me")]})),
            &host,
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SESSION_NOT_IN_PROGRESS");
}

#[tokio::test]
async fn stt_token_not_configured_and_issued() {
    let Some(c) = ctx(Arc::new(NoSttTokenIssuer), Arc::new(NoRateLimit)).await else {
        return;
    };
    let (_, host) = c.user(Some(WorkspaceRole::Member));
    let id = c.new_session(&host).await;
    let (s, b) = c
        .call(
            "POST",
            &format!("/copilot/sessions/{id}/stt-token"),
            None,
            &host,
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b["code"], "STT_NOT_CONFIGURED");

    let Some(c) = ctx(
        Arc::new(FakeIssuer),
        Arc::new(CountingLimiter(Mutex::new(0))),
    )
    .await
    else {
        return;
    };
    let (_, host) = c.user(Some(WorkspaceRole::Member));
    let (_, other) = c.user(Some(WorkspaceRole::Member));
    let id = c.new_session(&host).await;
    let uri = format!("/copilot/sessions/{id}/stt-token");
    let (s, _) = c.call("POST", &uri, None, &other).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, b) = c.call("POST", &uri, None, &host).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["provider"], "openai");
    assert_eq!(b["token"], format!("ek_{id}"));
    assert!(b["expiresAt"].is_string() && b["websocketUrl"].is_string());
    assert_eq!(b["sessionConfig"]["type"], "transcription");
    // 1 (host ok) so far; the limiter counts every allowed-check of host-only calls: 9 more pass.
    for _ in 0..9 {
        let (s, _) = c.call("POST", &uri, None, &host).await;
        assert_eq!(s, StatusCode::OK);
    }
    let (s, b) = c.call("POST", &uri, None, &host).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{b}");
}
