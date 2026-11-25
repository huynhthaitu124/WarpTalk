#![allow(clippy::unwrap_used)]

//! `ServerAudioStt` (the server-socket fallback client) against a local fake of the API's audio
//! socket, driven through the real engine.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

mod common;
use common::*;
use warptalk_copilot_core::{
    Channel, CopilotConfig, CopilotDeps, CopilotEngine, CopilotEvent, EngineExtras, FakeStt,
    LanguageMode, MemorySink, NoticeBoard, ServerAudioStt, SttRoute, SttRouting, TimingBoard,
};

#[derive(Default)]
struct Behavior {
    /// Answer the handshake with this status instead of upgrading.
    reject: Option<u16>,
    /// Send `closing` and hang up after the first utterance of a connection.
    close_after_first: bool,
    /// Reply to the first commit with a channel error instead of a segment.
    error_on_first_commit: bool,
    credits_flag: bool,
}

#[derive(Default)]
struct Fake {
    behavior: Mutex<Behavior>,
    bearers: Mutex<Vec<String>>,
    paths: Mutex<Vec<String>>,
    hellos: Mutex<Vec<Value>>,
    /// (channel byte, samples) per binary frame.
    frames: Mutex<Vec<(u8, usize)>>,
    commits: Mutex<Vec<Value>>,
    commits_seen: Mutex<u32>,
}

async fn handler(
    State(f): State<Arc<Fake>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    ws: WebSocketUpgrade,
) -> Response {
    f.bearers.lock().unwrap().push(
        headers
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default(),
    );
    f.paths.lock().unwrap().push(uri.path().to_owned());
    if let Some(code) = f.behavior.lock().unwrap().reject {
        return StatusCode::from_u16(code).unwrap().into_response();
    }
    ws.on_upgrade(move |s| session(f, s))
}

async fn session(f: Arc<Fake>, mut s: WebSocket) {
    let _ = s
        .send(Message::Text(
            json!({"type":"ready","connectionId":"c","sampleRate":16000,"maxFrameBytes":64000,"maxSessionSecs":3600})
                .to_string()
                .into(),
        ))
        .await;
    let mut local = 0u32;
    while let Some(Ok(m)) = s.recv().await {
        match m {
            Message::Binary(b) => f.frames.lock().unwrap().push((b[0], (b.len() - 1) / 2)),
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap() {
                    "hello" => f.hellos.lock().unwrap().push(v),
                    "commit" => {
                        let ch = v["channel"].as_str().unwrap().to_owned();
                        f.commits.lock().unwrap().push(v.clone());
                        let first = {
                            let mut n = f.commits_seen.lock().unwrap();
                            *n += 1;
                            *n == 1
                        };
                        let (err, credits, close_after) = {
                            let b = f.behavior.lock().unwrap();
                            (
                                b.error_on_first_commit && first,
                                b.credits_flag,
                                b.close_after_first,
                            )
                        };
                        local += 1;
                        let send = |x: Value| Message::Text(x.to_string().into());
                        if err {
                            let _ = s
                                .send(send(json!({"type":"error","code":"stt_failed","message":"boom","channel":ch,"fatal":false})))
                                .await;
                            let _ = s
                                .send(send(json!({"type":"utterance_done","channel":ch,"seq":v["seq"],"ok":false})))
                                .await;
                            continue;
                        }
                        let _ = s
                            .send(send(json!({
                                "type":"segment","channel":ch,"clientSegmentId":"x","segmentId":"y",
                                "text": format!("máy chủ nói {} trên {ch}", v["seq"]),
                                "language":"vi","startMs":v["startMs"],"endMs":v["endMs"],
                                "early":false,"duplicate":false,"insufficientCredits":credits
                            })))
                            .await;
                        let _ = s
                            .send(send(json!({"type":"utterance_done","channel":ch,"seq":v["seq"],"ok":true})))
                            .await;
                        if close_after && local == 1 {
                            let _ = s
                                .send(send(
                                    json!({"type":"closing","reason":"max_session_duration"}),
                                ))
                                .await;
                            let _ = s.send(Message::Close(None)).await;
                            return;
                        }
                    }
                    _ => {}
                }
            }
            Message::Close(_) => return,
            _ => {}
        }
    }
}

async fn serve(f: Arc<Fake>) -> String {
    let app = Router::new()
        .route("/api/v1/copilot/sessions/{id}/audio", get(handler))
        .with_state(f);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

struct Rig {
    e: CopilotEngine,
    rx: tokio::sync::mpsc::Receiver<CopilotEvent>,
    server: Arc<ServerAudioStt>,
    sink: Arc<MemorySink>,
    seen: Vec<CopilotEvent>,
}

fn rig(base: &str, mut cfg: CopilotConfig) -> Rig {
    cfg.stt.mode = SttRouting::Server;
    cfg.me.queue_capacity_chunks = 100_000;
    cfg.others.queue_capacity_chunks = 100_000;
    cfg.engine.status_interval_ms = 20;
    cfg.engine.reconnect_backoff_ms = 20;
    let timing = Arc::new(TimingBoard::default());
    let notices = Arc::new(NoticeBoard::default());
    let server = Arc::new(
        ServerAudioStt::new(
            cfg.stt.server.clone(),
            &cfg.language,
            base,
            "sess-1",
            "tok-1",
            timing.clone(),
            notices.clone(),
        )
        .unwrap(),
    );
    let sink = MemorySink::new();
    let (e, rx) = CopilotEngine::with_extras(
        cfg,
        CopilotDeps {
            stt: server.clone(),
            sink: sink.clone(),
            session_id: "sess-1".into(),
        },
        EngineExtras {
            server_stt: Some(server.clone()),
            timing: Some(timing),
            notices: Some(notices),
        },
    )
    .unwrap();
    Rig {
        e,
        rx,
        server,
        sink,
        seen: Vec::new(),
    }
}

impl Rig {
    async fn utterance(&mut self, ch: Channel, lead_ms: i64) {
        let audio = track(16_000, 0.3, &[-lead_ms, 800, -1500]);
        self.e.push_audio(ch, &audio, 16_000);
        let want = segments(&self.seen).len() + 1;
        let ev = collect_until(&mut self.rx, Duration::from_secs(5), |ev| {
            segments(ev).len() >= 1
        })
        .await;
        self.seen.extend(ev);
        assert!(segments(&self.seen).len() >= want, "{:#?}", self.seen);
    }

    async fn drain(&mut self, ms: u64) {
        self.seen
            .extend(collect_until(&mut self.rx, Duration::from_millis(ms), |_| false).await);
    }

    fn errors(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                CopilotEvent::Error { code, .. } => Some(code.clone()),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn speech_only_audio_goes_up_and_server_segments_are_shown_not_posted() {
    let f = Arc::new(Fake::default());
    let base = serve(f.clone()).await;
    let mut cfg = CopilotConfig::from_json(
        r#"{"language":{"mode":"auto","languages":["vi","en"],"vocabulary":["Kubernetes"]}}"#,
    )
    .unwrap();
    cfg.stt.server.event_timeout_ms = 2_000;
    let mut r = rig(&base, cfg);
    r.utterance(Channel::Me, 500).await;
    r.utterance(Channel::Others, 500).await;
    r.drain(100).await;

    // Auth + addressing + hello.
    assert_eq!(f.bearers.lock().unwrap()[0], "Bearer tok-1");
    assert_eq!(
        f.paths.lock().unwrap()[0],
        "/api/v1/copilot/sessions/sess-1/audio"
    );
    assert_eq!(
        f.bearers.lock().unwrap().len(),
        1,
        "one shared socket for both channels"
    );
    let hello = f.hellos.lock().unwrap()[0].clone();
    assert_eq!(hello["languageMode"], "auto");
    assert_eq!(hello["languages"], json!(["vi", "en"]));
    assert_eq!(hello["vocabulary"], json!(["Kubernetes"]));

    // Framing: channel byte per frame, and only the speech region (~800 ms plus the
    // endpointer's padding) was uploaded, never the 500 ms lead silence or the 1.5 s tail.
    let frames = f.frames.lock().unwrap().clone();
    for ch in [0u8, 1u8] {
        let samples: usize = frames
            .iter()
            .filter(|(c, _)| *c == ch)
            .map(|(_, n)| n)
            .sum();
        assert!(samples >= 16 * 800, "ch {ch}: {samples}");
        assert!(samples < 16 * 1900, "ch {ch} uploaded silence: {samples}");
    }
    assert!(frames.iter().all(|(_, n)| *n <= 16_000));

    // Commit carries the utterance position on the session timeline.
    let commits = f.commits.lock().unwrap().clone();
    assert_eq!(commits.len(), 2);
    let me = commits.iter().find(|c| c["channel"] == "me").unwrap();
    let (s, e) = (
        me["startMs"].as_i64().unwrap(),
        me["endMs"].as_i64().unwrap(),
    );
    assert!((s - 500).abs() < 150 && (e - 1300).abs() < 200, "{s}..{e}");
    assert_eq!(me["seq"], 0);

    // Shown, with the server's language, but never posted by the client.
    let segs = segments(&r.seen);
    assert_eq!(segs.len(), 2);
    assert!(
        segs.iter()
            .all(|s| s.language == "vi" && s.text.starts_with("máy chủ"))
    );
    assert!(r.sink.segments().is_empty());
    assert_eq!(r.e.stats().me.stt_route, SttRoute::Server);
    r.e.stop().await;
    assert!(r.sink.segments().is_empty());
}

#[tokio::test]
async fn a_closed_socket_is_replaced_transparently() {
    let f = Arc::new(Fake::default());
    f.behavior.lock().unwrap().close_after_first = true;
    let base = serve(f.clone()).await;
    let mut r = rig(&base, CopilotConfig::default());
    r.utterance(Channel::Me, 300).await;
    tokio::time::sleep(Duration::from_millis(100)).await; // the server hung up
    r.utterance(Channel::Me, 300).await;
    assert_eq!(r.server.connects(), 2, "reconnected once");
    assert_eq!(segments(&r.seen).len(), 2);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(
        f.hellos.lock().unwrap().len(),
        2,
        "hello on every connection"
    );
}

#[tokio::test]
async fn a_channel_error_fails_the_utterance_and_the_next_one_recovers() {
    let f = Arc::new(Fake::default());
    f.behavior.lock().unwrap().error_on_first_commit = true;
    let base = serve(f.clone()).await;
    let mut r = rig(&base, CopilotConfig::default());
    r.e.push_audio(
        Channel::Me,
        &track(16_000, 0.3, &[-300, 800, -1500]),
        16_000,
    );
    r.drain(400).await;
    assert!(
        r.errors().contains(&"stt_failed".to_string()),
        "{:?}",
        r.errors()
    );
    assert!(segments(&r.seen).is_empty());
    tokio::time::sleep(Duration::from_millis(60)).await; // reconnect backoff
    r.utterance(Channel::Me, 300).await;
    assert_eq!(segments(&r.seen).len(), 1);
}

#[tokio::test]
async fn handshake_rejection_reports_auth_required() {
    let f = Arc::new(Fake::default());
    f.behavior.lock().unwrap().reject = Some(401);
    let base = serve(f.clone()).await;
    let mut r = rig(&base, CopilotConfig::default());
    r.drain(300).await;
    assert!(
        r.errors().contains(&"auth_required".to_string()),
        "{:?}",
        r.errors()
    );
}

#[tokio::test]
async fn credit_exhaustion_from_the_server_surfaces_as_an_error_event() {
    let f = Arc::new(Fake::default());
    f.behavior.lock().unwrap().credits_flag = true;
    let base = serve(f.clone()).await;
    let mut r = rig(&base, CopilotConfig::default());
    r.utterance(Channel::Me, 300).await;
    r.drain(200).await;
    assert!(
        r.errors().contains(&"insufficient_credits".to_string()),
        "{:?}",
        r.errors()
    );
    assert_eq!(segments(&r.seen).len(), 1, "the transcript is still shown");
}

#[tokio::test]
async fn auto_falls_back_to_the_real_server_client() {
    // Direct always fails; the server socket (this fake) takes over.
    let f = Arc::new(Fake::default());
    let base = serve(f.clone()).await;
    let mut cfg = CopilotConfig::default();
    cfg.stt.fallback.failure_threshold = 2;
    cfg.stt.mode = SttRouting::Auto;
    cfg.language.mode = LanguageMode::Multi;
    cfg.me.queue_capacity_chunks = 100_000;
    cfg.engine.reconnect_backoff_ms = 20;
    let direct = FakeStt::new(Duration::ZERO);
    direct.fail_next_opens(1_000);
    let timing = Arc::new(TimingBoard::default());
    let notices = Arc::new(NoticeBoard::default());
    let server = Arc::new(
        ServerAudioStt::new(
            cfg.stt.server.clone(),
            &cfg.language,
            &base,
            "sess-1",
            "t",
            timing.clone(),
            notices.clone(),
        )
        .unwrap(),
    );
    let sink = MemorySink::new();
    let (e, mut rx) = CopilotEngine::with_extras(
        cfg,
        CopilotDeps {
            stt: Arc::new(direct.clone()),
            sink: sink.clone(),
            session_id: "sess-1".into(),
        },
        EngineExtras {
            server_stt: Some(server),
            timing: Some(timing),
            notices: Some(notices),
        },
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    e.push_audio(
        Channel::Me,
        &track(16_000, 0.3, &[-300, 800, -1500]),
        16_000,
    );
    let ev = collect_until(&mut rx, Duration::from_secs(5), |ev| {
        !segments(ev).is_empty()
    })
    .await;
    assert!(segments(&ev)[0].text.starts_with("máy chủ"));
    assert_eq!(e.stats().me.stt_route, SttRoute::Server);
    assert!(sink.segments().is_empty());
    assert_eq!(f.hellos.lock().unwrap()[0]["languageMode"], "multi");
    e.stop().await;
}
