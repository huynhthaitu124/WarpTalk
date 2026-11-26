#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use common::track;
use serde_json::Value;
use warptalk_copilot_core::{Channel, CopilotEvent, CopilotHandle, PushStatus, StartParams};

/// Sync facade end to end (fake STT, real HTTP sink against a local server).
#[test]
fn facade_start_push_events_stop() {
    let got: Arc<Mutex<Vec<Value>>> = Arc::default();
    let (tx, rx) = std::sync::mpsc::channel();
    let g = got.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let app = Router::new()
                .route(
                    "/api/v1/copilot/sessions/{id}/segments",
                    post(
                        |State(g): State<Arc<Mutex<Vec<Value>>>>, Json(b): Json<Value>| async move {
                            g.lock().unwrap().push(b);
                        },
                    ),
                )
                .with_state(g);
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap()).unwrap();
            axum::serve(l, app).await.unwrap();
        });
    });
    let addr = rx.recv().unwrap();

    let h =
        CopilotHandle::new(r#"{"stt":{"backend":"fake"},"sink":{"batch_window_ms":50}}"#.into())
            .unwrap();
    assert_eq!(
        h.push_audio(Channel::Me, vec![0.0; 320], 16_000),
        PushStatus::Closed,
        "not started"
    );
    h.start(StartParams {
        base_url: format!("http://{addr}"),
        session_id: "abc".into(),
        bearer: "tok".into(),
    })
    .unwrap();
    assert!(
        h.start(StartParams {
            base_url: String::new(),
            session_id: String::new(),
            bearer: String::new()
        })
        .is_err()
    );
    let audio = track(16_000, 0.3, &[-300, 1000, -2000]);
    for c in audio.chunks(320) {
        h.push_audio(Channel::Me, c.to_vec(), 16_000);
    }
    let ev = h.events();
    let mut seen = Vec::new();
    for _ in 0..200 {
        if let Some(e) = ev.next(50) {
            let done = matches!(e, CopilotEvent::Segment { .. });
            seen.push(e);
            if done {
                break;
            }
        }
    }
    assert!(seen.iter().any(
        |e| matches!(e, CopilotEvent::Segment { segment } if segment.text == "utterance 0 on me")
    ));
    assert_eq!(h.stats().unwrap().me.segments, 1);
    h.update_endpointer(Channel::Me, r#"{"vad_threshold":0.4}"#.into())
        .unwrap();
    assert!(h.update_endpointer(Channel::Me, "{".into()).is_err());
    h.stop().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let bodies = got.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["segments"][0]["text"], "utterance 0 on me");
    assert!(h.stop().is_err());
    assert!(h.stats().is_err());
}
