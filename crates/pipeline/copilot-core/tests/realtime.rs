#![allow(clippy::unwrap_used)]

//! Ephemeral-token realtime STT against a local fake of the OpenAI Realtime socket.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use warptalk_copilot_core::{
    CopilotError, EphemeralRealtimeStt, SttConfig, SttToken, SttTokenProvider, TokenManager,
};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttEvent, SttMode, Utterance,
};

#[derive(Default)]
struct Fake {
    bearers: Mutex<Vec<String>>,
    urls: Mutex<Vec<String>>,
    appended: AtomicU64,
    commits: AtomicU64,
}

async fn ws_handler(
    State(f): State<Arc<Fake>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    f.bearers.lock().unwrap().push(
        headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    f.urls.lock().unwrap().push(uri.to_string());
    ws.on_upgrade(move |s| session(f, s))
}

async fn session(f: Arc<Fake>, mut s: WebSocket) {
    let mut n = 0;
    while let Some(Ok(Message::Text(t))) = s.recv().await {
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap() {
            "input_audio_buffer.append" => {
                f.appended.fetch_add(1, Ordering::SeqCst);
            }
            "input_audio_buffer.commit" => {
                f.commits.fetch_add(1, Ordering::SeqCst);
                n += 1;
                let item = format!("item_{n}");
                for ev in [
                    json!({"type":"input_audio_buffer.committed","item_id":item}),
                    json!({"type":"conversation.item.input_audio_transcription.delta","item_id":item,"delta":"Hello there. "}),
                    json!({"type":"conversation.item.input_audio_transcription.delta","item_id":item,"delta":"General Kenobi"}),
                    json!({"type":"conversation.item.input_audio_transcription.completed","item_id":item,"transcript":"Hello there. General Kenobi"}),
                ] {
                    s.send(Message::Text(ev.to_string().into())).await.unwrap();
                }
            }
            _ => {}
        }
    }
}

struct Tokens {
    n: AtomicU64,
    ttl_ms: i64,
    ws_url: String,
}

#[async_trait]
impl SttTokenProvider for Tokens {
    async fn fetch(&self) -> Result<SttToken, CopilotError> {
        let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(SttToken {
            provider: "openai".into(),
            model: "gpt-live-transcribe".into(),
            token: format!("ephemeral-{n}"),
            expires_at: Utc::now() + chrono::Duration::milliseconds(self.ttl_ms),
            websocket_url: Some(self.ws_url.clone()),
        })
    }
}

fn utt(audio: Vec<f32>) -> CommitRequest {
    CommitRequest {
        utterance: Utterance {
            room: "r".into(),
            speaker: "me".into(),
            seq: 0,
            lang: None,
            audio: audio.into(),
            speech_ms: 500,
            total_ms: 500,
            hangover_ms: 0,
            reason: warptalk_ingress::EndReason::Silence,
            stamps: warptalk_pipeline_runtime::Stamps::new(std::time::Instant::now()),
        },
        mode: SttMode::Realtime,
        streamed_samples: 1,
    }
}

async fn run_one(stt: &EphemeralRealtimeStt) -> Vec<(String, bool)> {
    let (tx, mut rx) = mpsc::channel(16);
    let mut s = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "me".into(),
                lang_hint: Some("en".into()),
            },
            tx,
        )
        .await
        .unwrap();
    s.feed(&vec![0.1; 1600]).await.unwrap();
    s.commit(utt(vec![0.1; 1600])).await.unwrap();
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            SttEvent::Segment(g) => out.push((g.text, g.is_early)),
            SttEvent::UtteranceDone { ok } => {
                assert!(ok);
                break;
            }
        }
    }
    s.close().await;
    out
}

#[tokio::test]
async fn uses_ephemeral_token_url_and_refreshes_between_sessions() {
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route("/rt", get(ws_handler))
        .with_state(fake.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let prov = Arc::new(Tokens {
        n: AtomicU64::new(0),
        ttl_ms: 1500,
        ws_url: format!("ws://{addr}/rt"),
    });
    // margin 1 s of a 1.5 s token: still fresh for ~0.5 s.
    let mgr = TokenManager::new(prov.clone(), Duration::from_secs(1), true);
    let stt = EphemeralRealtimeStt::new(SttConfig::default(), mgr);

    let out = run_one(&stt).await;
    assert_eq!(
        out,
        vec![
            ("Hello there.".to_string(), true),
            ("General Kenobi".to_string(), false)
        ]
    );
    let out2 = run_one(&stt).await; // within freshness window: same token reused
    assert_eq!(out2.len(), 2);
    tokio::time::sleep(Duration::from_millis(700)).await; // now inside the refresh margin
    run_one(&stt).await;

    let bearers = fake.bearers.lock().unwrap().clone();
    assert_eq!(
        bearers,
        vec![
            "Bearer ephemeral-1",
            "Bearer ephemeral-1",
            "Bearer ephemeral-2"
        ],
        "token refreshed before expiry, never used after"
    );
    assert!(
        fake.urls
            .lock()
            .unwrap()
            .iter()
            .all(|u| u.contains("intent=transcription"))
    );
    assert_eq!(fake.commits.load(Ordering::SeqCst), 3);
    assert!(fake.appended.load(Ordering::SeqCst) >= 3);
}
