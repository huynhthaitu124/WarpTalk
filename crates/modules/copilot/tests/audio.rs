#![allow(clippy::unwrap_used)]
//! Server-side STT fallback socket against the local Postgres, in-memory fakes and a real TCP
//! listener (WebSocket upgrades cannot go through `oneshot`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_copilot::application::audio::{AudioConfig, ServerAudio};
use warptalk_copilot::application::ports::{
    CreatedRoom, FixedCreditGate, MemorySegmentSink, MemoryTicketStore, MemoryUsageSink, Metering,
    NewSessionRoom, NoRateLimit, NoSttTokenIssuer, RoomState, SessionRooms, WorkspaceMembers,
};
use warptalk_copilot::domain::Channel;
use warptalk_copilot::domain::audio::encode_audio_frame;
use warptalk_copilot::infrastructure::fake_stt::FakeServerStt;
use warptalk_copilot::{CopilotDeps, CopilotModule, MIGRATIONS};
use warptalk_kernel::{AppResult, Clock, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole};
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
        }
        Ok(())
    }
}

struct AllowAll;

#[async_trait]
impl WorkspaceMembers for AllowAll {
    async fn member_role(&self, _: WorkspaceId, _: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(Some(WorkspaceRole::Member))
    }
}

struct TestClock(Mutex<DateTime<Utc>>);

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

struct Env {
    addr: std::net::SocketAddr,
    app: axum::Router,
    keys: JwtKeys,
    sink: Arc<MemorySegmentSink>,
    usage: Arc<MemoryUsageSink>,
    gate: Arc<FixedCreditGate>,
    clock: Arc<TestClock>,
    stt: FakeServerStt,
    ws: WorkspaceId,
}

async fn env(cfg: AudioConfig, stt: FakeServerStt, configured: bool) -> Option<Env> {
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
    let sink = Arc::new(MemorySegmentSink::default());
    let usage = Arc::new(MemoryUsageSink::default());
    let gate = Arc::new(FixedCreditGate(Mutex::new(None)));
    let clock = Arc::new(TestClock(Mutex::new(Utc::now())));
    let mut module = CopilotModule::new(CopilotDeps {
        pool,
        jwt: keys.clone(),
        clock: clock.clone(),
        rooms: Arc::new(FakeRooms::default()),
        members: Arc::new(AllowAll),
        sink: sink.clone(),
        stt_tokens: Arc::new(NoSttTokenIssuer),
        rate_limiter: Arc::new(NoRateLimit),
    })
    .with_metering(Metering {
        usage: usage.clone(),
        gate: gate.clone(),
        stt_model: "fake-stt".into(),
    });
    if configured {
        module = module.with_server_stt(ServerAudio {
            stt: Arc::new(stt.clone()),
            tickets: Arc::new(MemoryTicketStore::default()),
            config: cfg,
            limiter: Arc::new(
                warptalk_copilot::application::audio::MemoryConnectionLimiter::default(),
            ),
        });
    }
    let app = module.router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Some(Env {
        addr,
        app,
        keys,
        sink,
        usage,
        gate,
        clock,
        stt,
        ws: WorkspaceId::new(),
    })
}

fn fast_cfg() -> AudioConfig {
    AudioConfig {
        drain_timeout_ms: 1_000,
        ..AudioConfig::default()
    }
}

impl Env {
    fn token(&self, user: UserId) -> String {
        self.keys
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
            .unwrap()
            .0
    }

    async fn http(
        &self,
        method: &str,
        uri: &str,
        tok: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let b = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {tok}"));
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
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

    async fn session(&self, user: UserId) -> String {
        let (s, b) = self
            .http(
                "POST",
                "/copilot/sessions",
                &self.token(user),
                Some(json!({
                    "workspaceId": self.ws.to_string(),
                    "platform": "zoom",
                    "languages": {"me": "vi", "others": "en"}
                })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        b["id"].as_str().unwrap().to_owned()
    }

    #[allow(clippy::result_large_err)]
    async fn connect_bearer(
        &self,
        session: &str,
        tok: &str,
    ) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
        let mut req = format!("ws://{}/copilot/sessions/{session}/audio", self.addr)
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {tok}").parse().unwrap());
        tokio_tungstenite::connect_async(req)
            .await
            .map(|(w, _)| Ws(w))
    }

    #[allow(clippy::result_large_err)]
    async fn connect_ticket(
        &self,
        session: &str,
        ticket: &str,
    ) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
        let url = format!(
            "ws://{}/copilot/sessions/{session}/audio?token={ticket}",
            self.addr
        );
        tokio_tungstenite::connect_async(url)
            .await
            .map(|(w, _)| Ws(w))
    }
}

fn status_of(e: tokio_tungstenite::tungstenite::Error) -> u16 {
    match e {
        tokio_tungstenite::tungstenite::Error::Http(r) => r.status().as_u16(),
        other => panic!("expected an http error, got {other:?}"),
    }
}

struct Ws(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
);

impl Ws {
    async fn send_json(&mut self, v: Value) {
        self.0
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    async fn send_audio(&mut self, ch: Channel, ms: usize) {
        let pcm = vec![1000i16; 16 * ms];
        self.0
            .send(Message::Binary(encode_audio_frame(ch, &pcm).into()))
            .await
            .unwrap();
    }

    async fn next_json(&mut self) -> Option<Value> {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.0.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    return Some(serde_json::from_str(t.as_str()).unwrap());
                }
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(_))) => continue,
                Err(_) => panic!("timed out waiting for a server message"),
            }
        }
    }

    /// Messages until one of `ty` (inclusive).
    async fn until(&mut self, ty: &str) -> Vec<Value> {
        let mut all = Vec::new();
        while let Some(m) = self.next_json().await {
            let hit = m["type"] == ty;
            all.push(m);
            if hit {
                return all;
            }
        }
        panic!("socket closed before `{ty}`; got {all:?}");
    }

    async fn utterance(&mut self, ch: &str, tag: Channel, seq: u64) {
        self.send_audio(tag, 200).await;
        self.send_json(json!({"type":"commit","channel":ch,"seq":seq,"startMs":1000,"endMs":1200}))
            .await;
    }
}

#[tokio::test]
async fn auth_is_required_before_the_upgrade() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    assert_eq!(
        status_of(e.connect_bearer(&id, "garbage").await.err().unwrap()),
        401
    );
    let no_auth = format!("ws://{}/copilot/sessions/{id}/audio", e.addr);
    assert_eq!(
        status_of(
            tokio_tungstenite::connect_async(no_auth)
                .await
                .err()
                .unwrap()
        ),
        401
    );
    assert_eq!(
        status_of(e.connect_ticket(&id, "nope").await.err().unwrap()),
        401
    );
    // Somebody else's bearer: not the host.
    let other = e.token(UserId::new());
    assert_eq!(
        status_of(e.connect_bearer(&id, &other).await.err().unwrap()),
        403
    );
}

#[tokio::test]
async fn not_configured_answers_503() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), false).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let (s, b) = e
        .http(
            "POST",
            &format!("/copilot/sessions/{id}/audio-ticket"),
            &e.token(host),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b["code"], "SERVER_STT_NOT_CONFIGURED");
    assert_eq!(
        status_of(e.connect_bearer(&id, &e.token(host)).await.err().unwrap()),
        503
    );
}

#[tokio::test]
async fn tickets_are_single_use_session_bound_and_expire() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let tok = e.token(host);
    let id = e.session(host).await;
    let id2 = e.session(host).await;
    let ticket_uri = format!("/copilot/sessions/{id}/audio-ticket");

    let (s, b) = e.http("POST", &ticket_uri, &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["ttlSeconds"], 60);
    let t1 = b["ticket"].as_str().unwrap().to_owned();
    // Bound to its session.
    assert_eq!(
        status_of(e.connect_ticket(&id2, &t1).await.err().unwrap()),
        401
    );
    // (that failed attempt consumed it; ask again)
    let t1 = e.http("POST", &ticket_uri, &tok, None).await.1["ticket"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut ws = e.connect_ticket(&id, &t1).await.unwrap();
    assert_eq!(ws.next_json().await.unwrap()["type"], "ready");
    // Single use.
    assert_eq!(
        status_of(e.connect_ticket(&id, &t1).await.err().unwrap()),
        401
    );

    // Expiry.
    let t2 = e.http("POST", &ticket_uri, &tok, None).await.1["ticket"]
        .as_str()
        .unwrap()
        .to_owned();
    *e.clock.0.lock().unwrap() += chrono::Duration::seconds(61);
    assert_eq!(
        status_of(e.connect_ticket(&id, &t2).await.err().unwrap()),
        401
    );
    // Only the host may ask.
    let (s, _) = e
        .http("POST", &ticket_uri, &e.token(UserId::new()), None)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn audio_becomes_idempotent_segments_with_channel_and_usage() {
    let Some(e) = env(
        AudioConfig {
            max_concurrent_per_user: 5,
            ..fast_cfg()
        },
        FakeServerStt::new(),
        true,
    )
    .await
    else {
        return;
    };
    let host = UserId::new();
    let tok = e.token(host);
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &tok).await.unwrap();
    let ready = ws.next_json().await.unwrap();
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["sampleRate"], 16000);
    ws.send_json(json!({"type":"hello","runId":"run-1","languageMode":"auto"}))
        .await;

    ws.utterance("me", Channel::Me, 0).await;
    let got = ws.until("utterance_done").await;
    let seg = got.iter().find(|m| m["type"] == "segment").unwrap();
    assert_eq!(seg["channel"], "me");
    assert_eq!(seg["text"], "utterance 0 on me");
    assert_eq!(seg["language"], "vi");
    assert_eq!(
        (seg["startMs"].as_i64(), seg["endMs"].as_i64()),
        (Some(1000), Some(1200))
    );
    assert_eq!(seg["duplicate"], false);
    assert_eq!(seg["insufficientCredits"], false);

    ws.utterance("others", Channel::Others, 0).await;
    let got = ws.until("utterance_done").await;
    let seg = got.iter().find(|m| m["type"] == "segment").unwrap();
    assert_eq!(seg["channel"], "others");
    assert_eq!(seg["language"], "en");

    {
        let results = e.sink.results.lock().unwrap();
        assert_eq!(results.len(), 2);
        let me = results
            .iter()
            .find(|r| r.channel.as_deref() == Some("me"))
            .unwrap();
        assert_eq!(me.text, "utterance 0 on me");
        assert_eq!(me.language, "vi");
        assert!(me.is_final_chunk);
        assert!(
            results
                .iter()
                .any(|r| r.channel.as_deref() == Some("others"))
        );
    }
    assert_eq!(e.usage.events.lock().unwrap().len(), 2);
    assert_eq!(e.usage.events.lock().unwrap()[0]["feature"], "stt");

    // A second socket of the same client run resends the same utterance: a duplicate.
    let mut ws2 = e.connect_bearer(&id, &tok).await.unwrap();
    ws2.next_json().await.unwrap();
    ws2.send_json(json!({"type":"hello","runId":"run-1"})).await;
    ws2.utterance("me", Channel::Me, 0).await;
    let got = ws2.until("utterance_done").await;
    assert_eq!(
        got.iter().find(|m| m["type"] == "segment").unwrap()["duplicate"],
        true
    );
    assert_eq!(e.sink.results.lock().unwrap().len(), 2, "no double write");
    assert_eq!(e.usage.events.lock().unwrap().len(), 2, "no double charge");

    // A different run id never collides.
    let mut ws3 = e.connect_bearer(&id, &tok).await.unwrap();
    ws3.next_json().await.unwrap();
    ws3.utterance("me", Channel::Me, 0).await;
    ws3.until("utterance_done").await;
    assert_eq!(e.sink.results.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn partials_and_early_segments_flow_back() {
    let Some(e) = env(
        fast_cfg(),
        FakeServerStt::new().with_early_sentences(2),
        true,
    )
    .await
    else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    ws.next_json().await.unwrap();
    ws.utterance("others", Channel::Others, 7).await;
    let got = ws.until("utterance_done").await;
    let segs: Vec<_> = got.iter().filter(|m| m["type"] == "segment").collect();
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0]["early"], true);
    assert_eq!(segs[2]["early"], false);
    assert_eq!(
        segs[2]["endMs"], 1200,
        "the last segment runs to the utterance end"
    );
    let ids: std::collections::HashSet<_> = segs
        .iter()
        .map(|s| s["clientSegmentId"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    let partials: Vec<_> = got.iter().filter(|m| m["type"] == "partial").collect();
    assert!(
        partials
            .iter()
            .any(|p| p["text"].as_str().unwrap().contains("early 0"))
    );
    assert!(
        partials.iter().any(|p| p["text"] == ""),
        "cleared at the end"
    );
    assert_eq!(e.sink.results.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn bad_frames_are_reported_not_fatal_until_the_limit() {
    let cfg = AudioConfig {
        max_frame_bytes: 1_000,
        max_bad_messages: 2,
        ..fast_cfg()
    };
    let Some(e) = env(cfg, FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    ws.next_json().await.unwrap();
    ws.0.send(Message::Binary(vec![9, 0, 0].into()))
        .await
        .unwrap();
    let m = ws.next_json().await.unwrap();
    assert_eq!(
        (m["type"].as_str(), m["code"].as_str(), m["fatal"].as_bool()),
        (Some("error"), Some("bad_frame"), Some(false))
    );
    ws.send_audio(Channel::Me, 100).await; // 3201 bytes > 1000
    assert_eq!(ws.next_json().await.unwrap()["code"], "bad_frame");
    ws.send_json(json!({"type":"nonsense"})).await;
    let m = ws.next_json().await.unwrap();
    assert_eq!(m["fatal"], true, "third bad message exceeds the limit");
    let closing = ws.until("closing").await;
    assert_eq!(closing.last().unwrap()["reason"], "bad_message");
    assert_eq!(e.stt.fed_samples(), 0);
}

#[tokio::test]
async fn commit_without_audio_is_rejected() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    ws.next_json().await.unwrap();
    ws.send_json(json!({"type":"commit","channel":"me","seq":0,"startMs":0,"endMs":10}))
        .await;
    let got = ws.until("error").await;
    assert_eq!(got[0]["type"], "utterance_done");
    assert_eq!(got[0]["ok"], false);
    assert_eq!(got[1]["code"], "empty_utterance");
    assert!(e.sink.results.lock().unwrap().is_empty());
}

#[tokio::test]
async fn per_user_concurrency_limit_and_release() {
    let cfg = AudioConfig {
        max_concurrent_per_user: 1,
        ..fast_cfg()
    };
    let Some(e) = env(cfg, FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let tok = e.token(host);
    let id = e.session(host).await;
    let mut a = e.connect_bearer(&id, &tok).await.unwrap();
    a.next_json().await.unwrap();
    assert_eq!(
        status_of(e.connect_bearer(&id, &tok).await.err().unwrap()),
        429
    );
    a.0.close(None).await.unwrap();
    drop(a);
    // The slot frees once the server notices the close.
    let mut ok = false;
    for _ in 0..50 {
        if e.connect_bearer(&id, &tok).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "permit released after the socket closed");
}

#[tokio::test]
async fn max_session_duration_closes_with_a_reason() {
    let cfg = AudioConfig {
        max_session_secs: 1,
        ..fast_cfg()
    };
    let Some(e) = env(cfg, FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    let got = ws.until("closing").await;
    assert_eq!(got.last().unwrap()["reason"], "max_session_duration");
    assert!(ws.next_json().await.is_none());
}

#[tokio::test]
async fn idle_socket_is_closed() {
    let cfg = AudioConfig {
        idle_timeout_secs: 1,
        ..fast_cfg()
    };
    let Some(e) = env(cfg, FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    let got = ws.until("closing").await;
    assert_eq!(got.last().unwrap()["reason"], "idle_timeout");
}

#[tokio::test]
async fn insufficient_credits_refuses_open_but_never_drops_a_running_transcript() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let tok = e.token(host);
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &tok).await.unwrap();
    ws.next_json().await.unwrap();

    // Suspended while connected: the segment is kept, flagged, and the client warned once.
    *e.gate.0.lock().unwrap() = Some("insufficient_credits".into());
    ws.utterance("me", Channel::Me, 0).await;
    let got = ws.until("utterance_done").await;
    let seg = got.iter().find(|m| m["type"] == "segment").unwrap();
    assert_eq!(seg["insufficientCredits"], true);
    assert!(
        got.iter()
            .any(|m| m["code"] == "insufficient_credits" && m["fatal"] == false)
    );
    assert_eq!(e.sink.results.lock().unwrap().len(), 1);

    // A new socket / ticket is refused like `stt-token`.
    let err = e.connect_bearer(&id, &tok).await.err().unwrap();
    assert_eq!(status_of(err), 402);
    let (s, b) = e
        .http(
            "POST",
            &format!("/copilot/sessions/{id}/audio-ticket"),
            &tok,
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 402);
    assert_eq!(b["code"], "INSUFFICIENT_CREDITS");
}

#[tokio::test]
async fn provider_open_failure_is_reported_and_recovers() {
    let stt = FakeServerStt::new();
    stt.fail_next_opens(1);
    let Some(e) = env(fast_cfg(), stt, true).await else {
        return;
    };
    let host = UserId::new();
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &e.token(host)).await.unwrap();
    ws.next_json().await.unwrap();
    ws.send_audio(Channel::Me, 100).await;
    let m = ws.next_json().await.unwrap();
    assert_eq!(
        (m["code"].as_str(), m["channel"].as_str()),
        (Some("stt_unavailable"), Some("me"))
    );
    // The next audio opens a fresh session.
    ws.utterance("me", Channel::Me, 0).await;
    ws.until("utterance_done").await;
    assert_eq!(e.sink.results.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ended_session_stops_the_socket() {
    let Some(e) = env(fast_cfg(), FakeServerStt::new(), true).await else {
        return;
    };
    let host = UserId::new();
    let tok = e.token(host);
    let id = e.session(host).await;
    let mut ws = e.connect_bearer(&id, &tok).await.unwrap();
    ws.next_json().await.unwrap();
    let (s, _) = e
        .http("POST", &format!("/copilot/sessions/{id}/end"), &tok, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    ws.utterance("me", Channel::Me, 0).await;
    let got = ws.until("closing").await;
    assert!(
        got.iter()
            .any(|m| m["code"] == "session_ended" && m["fatal"] == true)
    );
    assert!(e.sink.results.lock().unwrap().is_empty());
    // And new sockets are refused (409).
    assert_eq!(
        status_of(e.connect_bearer(&id, &tok).await.err().unwrap()),
        409
    );
    let _ = SystemClock;
}
