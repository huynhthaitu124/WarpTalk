//! Server-side STT fallback end to end: the REAL API router (served on TCP) with Postgres +
//! Redis, a fake provider behind the `ServerStt` port, and the REAL `copilot-core` client
//! (`CopilotHandle`) on the other side of the audio socket. What lands in `stt:results` is
//! checked per channel, for idempotency and for "no double posting" in auto fallback.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_bus::StreamMessage;
use warptalk_config::JwtConfig;
use warptalk_contracts::events::SttResult;
use warptalk_copilot::application::audio::AudioConfig;
use warptalk_copilot::application::ports::NoSttTokenIssuer;
use warptalk_copilot::infrastructure::fake_stt::FakeServerStt;
use warptalk_copilot_core::{Channel, CopilotEvent, CopilotHandle, StartParams, SttRoute};
use warptalk_identity::VerificationMailer;
use warptalk_kernel::AppError;
use warptalk_web::JwtKeys;

#[derive(Default)]
struct CaptureMailer(Mutex<Vec<(String, String)>>);

#[async_trait]
impl VerificationMailer for CaptureMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push((email.to_owned(), token.to_owned()));
        Ok(())
    }
}

struct Env {
    app: axum::Router,
    addr: std::net::SocketAddr,
    redis: redis::aio::ConnectionManager,
    mailer: Arc<CaptureMailer>,
}

fn migrations() -> Vec<warptalk_db::ModuleMigrations> {
    vec![
        warptalk_db::OUTBOX_MIGRATIONS,
        warptalk_settings::MIGRATIONS,
        warptalk_identity::MIGRATIONS,
        warptalk_workspace::MIGRATIONS,
        warptalk_room::MIGRATIONS,
        warptalk_meeting::MIGRATIONS,
        warptalk_transcript::MIGRATIONS,
        warptalk_knowledge::MIGRATIONS,
        warptalk_assistant::MIGRATIONS,
        warptalk_copilot::MIGRATIONS,
        warptalk_billing::MIGRATIONS,
    ]
}

async fn env() -> Option<Env> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        eprintln!("DATABASE_URL / REDIS_URL not set; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 8).await.unwrap();
    warptalk_db::run_migrations(&pool, &migrations())
        .await
        .unwrap();
    let cfg = JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    };
    let jwt = JwtKeys::from_config(&cfg);
    let mailer = Arc::new(CaptureMailer::default());
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let app = warptalk_api::build_router(
        pool,
        jwt,
        &cfg,
        &[],
        mailer.clone(),
        Some(redis.clone()),
        warptalk_api::ApiOptions {
            // Direct STT is unavailable (no platform key): the token endpoint answers 503.
            stt_token_issuer: Some(Arc::new(NoSttTokenIssuer)),
            copilot_server_stt: Some(Arc::new(FakeServerStt::new())),
            copilot_audio_config: Some(AudioConfig {
                drain_timeout_ms: 1_000,
                ..AudioConfig::default()
            }),
            billing_config: Some(warptalk_billing::application::service::BillingConfig {
                webhook_secret: Some("whsec_test".into()),
                ..Default::default()
            }),
            ..warptalk_api::ApiOptions::default()
        },
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Some(Env {
        app,
        addr,
        redis,
        mailer,
    })
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/v1{uri}"));
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

impl Env {
    /// Registers, verifies and logs in a fresh user; returns the access token.
    async fn user(&self) -> String {
        let email = format!("cpa-{}@example.test", Uuid::new_v4().simple());
        let pw = "S3cret-pass!";
        let (s, b) = call(
            &self.app,
            "POST",
            "/auth/register",
            Some(json!({"email": email, "password": pw, "fullName": "Host"})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        let token = self
            .mailer
            .0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(e, _)| *e == email)
            .map(|(_, t)| t.clone())
            .unwrap();
        let (s, _) = call(
            &self.app,
            "POST",
            "/auth/verify-email",
            Some(json!({"token": token})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, b) = call(
            &self.app,
            "POST",
            "/auth/login",
            Some(json!({"email": email, "password": pw})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        b["accessToken"].as_str().unwrap().to_owned()
    }

    async fn session(&self, tok: &str) -> String {
        let (s, b) = call(
            &self.app,
            "POST",
            "/workspaces",
            Some(json!({"name": format!("CPA {}", Uuid::new_v4().simple())})),
            Some(tok),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        let ws = b["id"].as_str().unwrap().to_owned();
        let (s, b) = call(
            &self.app,
            "POST",
            "/copilot/sessions",
            Some(json!({"workspaceId": ws, "platform": "zoom", "languages": {"me": "vi", "others": "en"}})),
            Some(tok),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        b["id"].as_str().unwrap().to_owned()
    }

    async fn results(&self, session: &str) -> Vec<SttResult> {
        let mut conn = self.redis.clone();
        let reply: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
            .arg(format!("stt:results:{session}"))
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        reply
            .ids
            .into_iter()
            .filter_map(|e| {
                StreamMessage {
                    stream: String::new(),
                    id: e.id.clone(),
                    fields: e
                        .map
                        .iter()
                        .filter_map(|(k, v)| {
                            Some((k.clone(), redis::from_redis_value::<String>(v).ok()?))
                        })
                        .collect::<HashMap<_, _>>(),
                }
                .json::<SttResult>()
                .ok()
            })
            .collect()
    }
}

fn pcm_frame(ch: u8, ms: usize) -> Message {
    let mut f = vec![ch];
    for _ in 0..16 * ms {
        f.extend_from_slice(&1000i16.to_le_bytes());
    }
    Message::Binary(f.into())
}

async fn next_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            _ => continue,
        }
    }
}

#[tokio::test]
async fn ticket_socket_lands_idempotent_segments_in_stt_results() {
    let Some(e) = env().await else { return };
    let tok = e.user().await;
    let id = e.session(&tok).await;

    // Ticket flow (what a WKWebView client does): POST, then ?token=.
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/copilot/sessions/{id}/audio-ticket"),
        None,
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let ticket = b["ticket"].as_str().unwrap().to_owned();
    let url = format!(
        "ws://{}/api/v1/copilot/sessions/{id}/audio?token={ticket}",
        e.addr
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    assert_eq!(next_json(&mut ws).await["type"], "ready");
    // The ticket is single use.
    assert!(tokio_tungstenite::connect_async(&url).await.is_err());

    ws.send(Message::Text(
        json!({"type":"hello","runId":"e2e-run"}).to_string().into(),
    ))
    .await
    .unwrap();
    for (ch, name) in [(0u8, "me"), (1u8, "others")] {
        ws.send(pcm_frame(ch, 300)).await.unwrap();
        ws.send(Message::Text(
            json!({"type":"commit","channel":name,"seq":0,"startMs":1000,"endMs":1300})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        loop {
            let m = next_json(&mut ws).await;
            if m["type"] == "utterance_done" {
                break;
            }
        }
    }
    let r = e.results(&id).await;
    assert_eq!(r.len(), 2);
    let me = r
        .iter()
        .find(|x| x.channel.as_deref() == Some("me"))
        .unwrap();
    assert_eq!(
        (me.text.as_str(), me.language.as_str()),
        ("utterance 0 on me", "vi")
    );
    assert_eq!((me.start_ms, me.end_ms), (1000, 1300));
    assert!(
        r.iter()
            .any(|x| x.channel.as_deref() == Some("others") && x.language == "en")
    );

    // Same run id on a new socket: idempotent.
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/copilot/sessions/{id}/audio-ticket"),
        None,
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let url2 = format!(
        "ws://{}/api/v1/copilot/sessions/{id}/audio?token={}",
        e.addr,
        b["ticket"].as_str().unwrap()
    );
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url2).await.unwrap();
    next_json(&mut ws2).await;
    ws2.send(Message::Text(
        json!({"type":"hello","runId":"e2e-run"}).to_string().into(),
    ))
    .await
    .unwrap();
    ws2.send(pcm_frame(0, 300)).await.unwrap();
    ws2.send(Message::Text(
        json!({"type":"commit","channel":"me","seq":0,"startMs":1000,"endMs":1300})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    loop {
        let m = next_json(&mut ws2).await;
        if m["type"] == "segment" {
            assert_eq!(m["duplicate"], true);
        }
        if m["type"] == "utterance_done" {
            break;
        }
    }
    assert_eq!(e.results(&id).await.len(), 2, "duplicate not re-published");
}

/// Runs `f` against a fresh `CopilotHandle` on a blocking thread (it owns a runtime).
async fn with_handle<T: Send + 'static>(
    cfg: &'static str,
    addr: std::net::SocketAddr,
    id: String,
    tok: String,
    f: impl FnOnce(&CopilotHandle) -> T + Send + 'static,
) -> T {
    tokio::task::spawn_blocking(move || {
        let h = CopilotHandle::new(cfg.into()).unwrap();
        h.start(StartParams {
            base_url: format!("http://{addr}"),
            session_id: id,
            bearer: tok,
        })
        .unwrap();
        let out = f(&h);
        h.stop().unwrap();
        out
    })
    .await
    .unwrap()
}

fn speak(h: &CopilotHandle, ch: Channel) {
    // 300 ms lead, 800 ms of tone, 1.5 s of silence to endpoint.
    let mut a = vec![0.0f32; 16 * 300];
    a.extend(
        (0..16 * 800)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin()),
    );
    a.extend(vec![0.0f32; 16 * 1500]);
    h.push_audio(ch, a, 16_000);
}

fn wait_segments(h: &CopilotHandle, n: usize) -> Vec<CopilotEvent> {
    let stream = h.events();
    let mut all = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(8) {
        if let Some(ev) = stream.next(100) {
            all.push(ev);
        }
        if all
            .iter()
            .filter(|e| matches!(e, CopilotEvent::Segment { .. }))
            .count()
            >= n
        {
            break;
        }
    }
    all
}

#[tokio::test]
async fn real_client_in_server_mode_streams_audio_and_the_server_ingests() {
    let Some(e) = env().await else { return };
    let tok = e.user().await;
    let id = e.session(&tok).await;
    let evs = with_handle(
        r#"{"stt":{"mode":"server"},"language":{"mode":"auto","languages":["vi","en"]}}"#,
        e.addr,
        id.clone(),
        tok.clone(),
        |h| {
            speak(h, Channel::Me);
            speak(h, Channel::Others);
            let evs = wait_segments(h, 2);
            // Give the server a moment to finish ingesting before stop().
            let st = h.stats().unwrap();
            assert_eq!(st.me.stt_route, SttRoute::Server);
            evs
        },
    )
    .await;
    let shown: Vec<_> = evs
        .iter()
        .filter(|e| matches!(e, CopilotEvent::Segment { .. }))
        .collect();
    assert_eq!(shown.len(), 2, "{evs:#?}");
    let r = e.results(&id).await;
    // One per channel: the client did not also post them (no double posting).
    assert_eq!(r.len(), 2, "{r:#?}");
    assert!(r.iter().any(|x| x.channel.as_deref() == Some("me")));
    assert!(r.iter().any(|x| x.channel.as_deref() == Some("others")));
}

#[tokio::test]
async fn auto_mode_falls_back_when_the_token_endpoint_is_down() {
    let Some(e) = env().await else { return };
    let tok = e.user().await;
    let id = e.session(&tok).await;
    let evs = with_handle(
        r#"{"stt":{"mode":"auto","fallback":{"failure_threshold":2},"open_timeout_ms":3000},
            "engine":{"reconnect_backoff_ms":50}}"#,
        e.addr,
        id.clone(),
        tok.clone(),
        |h| {
            // The start-up attempt failed (stt-token answers 503); the next opens at speech start.
            std::thread::sleep(Duration::from_millis(150));
            speak(h, Channel::Me);
            let evs = wait_segments(h, 1);
            assert_eq!(h.stats().unwrap().me.stt_route, SttRoute::Server);
            evs
        },
    )
    .await;
    let fell_back = evs.iter().any(|e| {
        matches!(
            e,
            CopilotEvent::Status {
                kind: warptalk_copilot_core::StatusKind::SttFallback,
                ..
            }
        )
    });
    assert!(fell_back, "{evs:#?}");
    let r = e.results(&id).await;
    assert_eq!(r.len(), 1, "exactly once in stt:results: {r:#?}");
    assert_eq!(r[0].channel.as_deref(), Some("me"));
}
