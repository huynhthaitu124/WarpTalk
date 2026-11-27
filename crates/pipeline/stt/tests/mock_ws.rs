#![allow(clippy::unwrap_used, clippy::result_large_err)]
//! Pool and session tests against a local mock Realtime server. No real network.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;
use warptalk_pipeline_runtime::testing::test_utterance;
use warptalk_pipeline_runtime::{CommitRequest, SessionParams, SpeechToText, SttEvent, SttMode};
use warptalk_stt::{OpenAiRealtimeStt, OpenAiSttConfig};

#[derive(Default)]
struct ServerState {
    accepted: AtomicUsize,
    auth_headers: Mutex<Vec<String>>,
    /// Every client event received, parsed.
    received: Mutex<Vec<Value>>,
    /// Close the first N connections right after the handshake.
    close_first: AtomicUsize,
}

async fn spawn_server(state: Arc<ServerState>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let st = state.clone();
                let cb = move |req: &Request, resp: Response| {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    st.auth_headers.lock().unwrap().push(auth);
                    assert!(
                        req.uri()
                            .query()
                            .unwrap_or("")
                            .contains("intent=transcription")
                    );
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await else {
                    return;
                };
                let n = state.accepted.fetch_add(1, Ordering::SeqCst);
                if n < state.close_first.load(Ordering::SeqCst) {
                    let _ = ws.close(None).await;
                    return;
                }
                let mut item = 0u32;
                let mut appended_bytes = 0usize;
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(t) = msg else { continue };
                    let v: Value = serde_json::from_str(&t).unwrap();
                    state.received.lock().unwrap().push(v.clone());
                    match v["type"].as_str().unwrap() {
                        "input_audio_buffer.append" => {
                            appended_bytes += v["audio"].as_str().unwrap().len();
                            // Streaming model: words appear while audio arrives.
                            if appended_bytes > 0 && item == 0 {
                                let d = json!({"type":"conversation.item.input_audio_transcription.delta",
                                    "item_id":"item_1","delta":"Hello there. How are"});
                                ws.send(Message::Text(d.to_string().into())).await.unwrap();
                                item = 1;
                            }
                        }
                        "input_audio_buffer.commit" => {
                            for e in [
                                json!({"type":"input_audio_buffer.committed","item_id":"item_1"}),
                                json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":" you today? Thanks"}),
                                json!({"type":"conversation.item.input_audio_transcription.completed",
                                    "item_id":"item_1","transcript":"Hello there. How are you today? Thanks"}),
                            ] {
                                ws.send(Message::Text(e.to_string().into())).await.unwrap();
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    format!("ws://{addr}/v1/realtime")
}

fn cfg(url: &str, pool: usize) -> OpenAiSttConfig {
    OpenAiSttConfig {
        api_key: "test-key".into(),
        base_url: url.into(),
        pool_size: pool,
        event_timeout_ms: 2_000,
        ..OpenAiSttConfig::default()
    }
}

async fn wait_for(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn pool_warms_claims_instantly_and_refills() {
    let state = Arc::new(ServerState::default());
    let url = spawn_server(state.clone()).await;
    let stt = OpenAiRealtimeStt::new(cfg(&url, 4));
    assert_eq!(stt.warm_up().await, 4);
    assert_eq!(stt.pool().stats().idle, 4);
    assert!(
        state
            .auth_headers
            .lock()
            .unwrap()
            .iter()
            .all(|h| h == "Bearer test-key")
    );

    let (tx, _rx) = mpsc::channel(16);
    let _s = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "s".into(),
                lang_hint: Some("vi".into()),
            },
            tx,
        )
        .await
        .unwrap();
    let stats = stt.pool().stats();
    assert_eq!(stats.warm_claims, 1);
    assert_eq!(stats.cold_connects, 0, "claim must not connect");
    // Background refill brings the server to 5 accepted connections and the pool back to 4.
    wait_for(|| state.accepted.load(Ordering::SeqCst) == 5).await;
    wait_for(|| stt.pool().stats().idle == 4).await;
}

#[tokio::test]
async fn dead_warm_sockets_are_discarded_then_cold_connect() {
    let state = Arc::new(ServerState::default());
    state.close_first.store(2, Ordering::SeqCst);
    let url = spawn_server(state.clone()).await;
    let stt = OpenAiRealtimeStt::new(cfg(&url, 2));
    stt.warm_up().await;
    // Let the server's close frames arrive.
    wait_for(|| state.accepted.load(Ordering::SeqCst) == 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (tx, _rx) = mpsc::channel(16);
    let _s = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "s".into(),
                lang_hint: None,
            },
            tx,
        )
        .await
        .unwrap();
    let stats = stt.pool().stats();
    assert_eq!(stats.discarded, 2);
    assert_eq!(stats.cold_connects, 1);
}

#[tokio::test]
async fn missing_api_key_is_a_clean_error() {
    let stt = OpenAiRealtimeStt::new(OpenAiSttConfig::default());
    let (tx, _rx) = mpsc::channel(1);
    let r = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "s".into(),
                lang_hint: None,
            },
            tx,
        )
        .await;
    assert!(r.is_err());
}

#[tokio::test]
async fn end_to_end_flash_session_streams_then_commits() {
    let state = Arc::new(ServerState::default());
    let url = spawn_server(state.clone()).await;
    let stt = OpenAiRealtimeStt::new(cfg(&url, 1));
    stt.warm_up().await;

    let (tx, mut rx) = mpsc::channel(16);
    let mut s = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "s".into(),
                lang_hint: Some("en".into()),
            },
            tx,
        )
        .await
        .unwrap();

    let frame = vec![0.1f32; 512];
    s.feed(&frame).await.unwrap();
    s.feed(&frame).await.unwrap();
    let mut utt = test_utterance("r", "s", 0, vec![0.1; 1024]);
    utt.lang = Some("en".into());
    s.commit(CommitRequest {
        utterance: utt,
        mode: SttMode::Realtime,
        streamed_samples: 1024,
    })
    .await
    .unwrap();

    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(ev, SttEvent::UtteranceDone { .. });
        events.push(ev);
        if done {
            break;
        }
    }
    let texts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SttEvent::Segment(s) => Some((s.text.clone(), s.is_early)),
            SttEvent::UtteranceDone { .. } | SttEvent::Partial { .. } => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            ("Hello there.".to_string(), true),
            ("How are you today?".to_string(), true),
            ("Thanks".to_string(), false),
        ]
    );
    assert!(matches!(
        events.last(),
        Some(SttEvent::UtteranceDone { ok: true })
    ));

    // What the server received: session.update first, pcm16@24k audio, then commit (no resend).
    let recv = state.received.lock().unwrap().clone();
    assert_eq!(recv[0]["type"], "session.update");
    assert_eq!(recv[0]["session"]["type"], "transcription");
    assert_eq!(
        recv[0]["session"]["audio"]["input"]["format"]["rate"],
        24000
    );
    assert_eq!(
        recv[0]["session"]["audio"]["input"]["transcription"]["model"],
        "gpt-live-transcribe"
    );
    assert!(recv[0]["session"]["audio"]["input"]["turn_detection"].is_null());
    let appends: Vec<_> = recv
        .iter()
        .filter(|v| v["type"] == "input_audio_buffer.append")
        .collect();
    assert_eq!(
        appends.len(),
        2,
        "streamed frames are not sent again at commit"
    );
    // 512 samples @16k -> 768 @24k -> 1536 bytes -> 2048 base64 chars.
    assert_eq!(appends[0]["audio"].as_str().unwrap().len(), 2048);
    assert_eq!(recv.last().unwrap()["type"], "input_audio_buffer.commit");

    s.close().await;
}

#[tokio::test]
async fn closed_chunk_commit_sends_audio_first() {
    let state = Arc::new(ServerState::default());
    let url = spawn_server(state.clone()).await;
    let stt = OpenAiRealtimeStt::new(cfg(&url, 1));
    let (tx, mut rx) = mpsc::channel(16);
    let mut s = stt
        .open_session(
            SessionParams {
                room: "r".into(),
                speaker: "s".into(),
                lang_hint: None,
            },
            tx,
        )
        .await
        .unwrap();
    s.commit(CommitRequest {
        utterance: test_utterance("r", "s", 0, vec![0.1; 1600]),
        mode: SttMode::ClosedChunk,
        streamed_samples: 0,
    })
    .await
    .unwrap();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(ev, SttEvent::UtteranceDone { .. }) {
            break;
        }
    }
    let types: Vec<String> = state
        .received
        .lock()
        .unwrap()
        .iter()
        .map(|v| v["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        types,
        [
            "session.update",
            "input_audio_buffer.append",
            "input_audio_buffer.commit"
        ]
    );
    s.close().await;
}
