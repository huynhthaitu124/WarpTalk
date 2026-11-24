#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use warptalk_copilot_core::{Channel, HttpSegmentSink, SegmentRecord, SegmentSink, SinkConfig};

#[derive(Default)]
struct Server {
    /// Fail this many next requests with 500.
    fail_next: AtomicU64,
    down: AtomicBool,
    reject: AtomicBool,
    reject_code: AtomicU64,
    requests: Mutex<Vec<(String, String, Value)>>, // (session, auth, body)
    accepted: Mutex<Vec<String>>,                  // client ids accepted (2xx), dedup-able
}

async fn segments(
    State(s): State<Arc<Server>>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    s.requests.lock().unwrap().push((sid, auth, body.clone()));
    if s.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if s.reject.load(Ordering::SeqCst) {
        let c = s.reject_code.load(Ordering::SeqCst) as u16;
        return StatusCode::from_u16(if c == 0 { 422 } else { c }).unwrap();
    }
    if s.fail_next
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
        .is_ok()
    {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    let mut acc = s.accepted.lock().unwrap();
    for seg in body["segments"].as_array().unwrap() {
        acc.push(seg["clientSegmentId"].as_str().unwrap().to_string());
    }
    StatusCode::CREATED
}

async fn serve() -> (Arc<Server>, String) {
    let s = Arc::new(Server::default());
    let app = Router::new()
        .route("/api/v1/copilot/sessions/{id}/segments", post(segments))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (s, format!("http://{addr}"))
}

fn cfg() -> SinkConfig {
    SinkConfig {
        batch_max: 3,
        batch_window_ms: 80,
        retry_initial_ms: 20,
        retry_max_ms: 60,
        flush_timeout_ms: 5_000,
        ..SinkConfig::default()
    }
}

fn seg(i: u64) -> SegmentRecord {
    SegmentRecord {
        client_segment_id: SegmentRecord::new_id(),
        channel: if i.is_multiple_of(2) {
            Channel::Me
        } else {
            Channel::Others
        },
        text: format!("text {i}"),
        language: "en".into(),
        start_ms: i * 1000,
        end_ms: i * 1000 + 500,
        confidence: (i == 0).then_some(0.9),
        is_final: true,
        likely_echo: true, // must never reach the wire
        utterance_seq: i,
    }
}

#[tokio::test]
async fn batches_with_wire_format_and_bearer() {
    let (srv, base) = serve().await;
    let sink = HttpSegmentSink::spawn(cfg(), &base, "sess-9", "BEARER-1").unwrap();
    let all: Vec<_> = (0..5).map(seg).collect();
    for s in &all {
        sink.submit(s.clone()).await;
    }
    sink.flush().await.unwrap();
    let reqs = srv.requests.lock().unwrap().clone();
    let sizes: Vec<_> = reqs
        .iter()
        .map(|r| r.2["segments"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, vec![3, 2], "batch_max=3: {sizes:?}");
    assert!(
        reqs.iter()
            .all(|r| r.0 == "sess-9" && r.1 == "Bearer BEARER-1")
    );
    let first = &reqs[0].2["segments"][0];
    assert_eq!(first["clientSegmentId"], all[0].client_segment_id);
    assert_eq!(first["channel"], "me");
    assert_eq!(first["text"], "text 0");
    assert_eq!(first["language"], "en");
    assert_eq!(first["startMs"], 0);
    assert_eq!(first["endMs"], 500);
    assert!((first["confidence"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    assert_eq!(first["isFinal"], true);
    assert!(first.get("likelyEcho").is_none() && first.get("likely_echo").is_none());
    assert!(reqs[0].2["segments"][1].get("confidence").is_none());
    let st = sink.stats();
    assert_eq!((st.submitted, st.delivered, st.buffered), (5, 5, 0));
    sink.close().await;
}

#[tokio::test]
async fn retries_with_same_ids_and_loses_nothing() {
    let (srv, base) = serve().await;
    srv.fail_next.store(3, Ordering::SeqCst);
    let sink = HttpSegmentSink::spawn(cfg(), &base, "s", "t").unwrap();
    let all: Vec<_> = (0..3).map(seg).collect();
    for s in &all {
        sink.submit(s.clone()).await;
    }
    sink.flush().await.unwrap();
    let reqs = srv.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 4, "3 failures + 1 success");
    // Every attempt carried the identical ids in the identical order (idempotent retry).
    let ids = |v: &Value| -> Vec<String> {
        v["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["clientSegmentId"].as_str().unwrap().to_string())
            .collect()
    };
    let want: Vec<_> = all.iter().map(|s| s.client_segment_id.clone()).collect();
    for r in &reqs {
        assert_eq!(ids(&r.2), want);
    }
    assert_eq!(*srv.accepted.lock().unwrap(), want);
    assert!(sink.stats().retries >= 3);
    assert!(!sink.stats().offline);
    sink.close().await;
}

#[tokio::test]
async fn offline_buffer_is_bounded_and_flushes_on_reconnect() {
    let (srv, base) = serve().await;
    srv.down.store(true, Ordering::SeqCst);
    let mut c = cfg();
    c.max_buffered_segments = 4;
    c.batch_max = 10;
    let sink = HttpSegmentSink::spawn(c, &base, "s", "t").unwrap();
    let all: Vec<_> = (0..6).map(seg).collect();
    for s in &all {
        sink.submit(s.clone()).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let st = sink.stats();
    assert!(st.offline, "{st:?}");
    assert_eq!(st.buffered, 4);
    assert_eq!(st.dropped_overflow, 2);
    assert_eq!(st.delivered, 0);
    assert!(srv.accepted.lock().unwrap().is_empty());
    srv.down.store(false, Ordering::SeqCst);
    sink.flush().await.unwrap();
    let want: Vec<_> = all[2..]
        .iter()
        .map(|s| s.client_segment_id.clone())
        .collect();
    assert_eq!(
        *srv.accepted.lock().unwrap(),
        want,
        "oldest two dropped, order kept"
    );
    let st = sink.stats();
    assert!(!st.offline);
    assert_eq!((st.delivered, st.buffered), (4, 0));
    sink.close().await;
}

#[tokio::test]
async fn flush_reports_timeout_while_offline_and_keeps_buffer() {
    let (srv, base) = serve().await;
    srv.down.store(true, Ordering::SeqCst);
    let mut c = cfg();
    c.flush_timeout_ms = 150;
    let sink = HttpSegmentSink::spawn(c, &base, "s", "t").unwrap();
    sink.submit(seg(0)).await;
    assert!(sink.flush().await.is_err());
    assert_eq!(sink.stats().buffered, 1);
    srv.down.store(false, Ordering::SeqCst);
    sink.flush().await.unwrap();
    assert_eq!(srv.accepted.lock().unwrap().len(), 1);
    sink.close().await;
}

#[tokio::test]
async fn permanently_rejected_batch_is_dropped_not_retried_forever() {
    let (srv, base) = serve().await;
    srv.reject.store(true, Ordering::SeqCst);
    let sink = HttpSegmentSink::spawn(cfg(), &base, "s", "t").unwrap();
    sink.submit(seg(0)).await;
    sink.flush().await.unwrap();
    let st = sink.stats();
    assert_eq!((st.rejected, st.buffered), (1, 0));
    assert_eq!(srv.requests.lock().unwrap().len(), 1);
    sink.close().await;
}

#[tokio::test]
async fn unreachable_server_buffers_then_recovers_after_bearer_update() {
    // No server at all (closed port): submit must not fail or block.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let sink =
        HttpSegmentSink::spawn(cfg(), &format!("http://127.0.0.1:{port}"), "s", "t").unwrap();
    let t0 = std::time::Instant::now();
    sink.submit(seg(0)).await;
    assert!(t0.elapsed() < Duration::from_millis(50));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sink.stats().offline);
    assert_eq!(sink.stats().buffered, 1);
    sink.set_bearer("rotated");
}

#[tokio::test]
async fn stops_retrying_on_409_403_404_and_sanitizes_to_server_limits() {
    for code in [409u64, 403, 404] {
        let (srv, base) = serve().await;
        srv.reject.store(true, Ordering::SeqCst);
        srv.reject_code.store(code, Ordering::SeqCst);
        let sink = HttpSegmentSink::spawn(cfg(), &base, "s", "t").unwrap();
        sink.submit(seg(0)).await;
        sink.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            srv.requests.lock().unwrap().len(),
            1,
            "code {code} must not be retried"
        );
        assert_eq!(sink.stats().rejected, 1);
        sink.close().await;
    }
    let (srv, base) = serve().await;
    let sink = HttpSegmentSink::spawn(cfg(), &base, "s", "t").unwrap();
    let mut long = seg(1);
    long.text = "x".repeat(5000);
    long.start_ms = u64::MAX;
    long.end_ms = 5;
    long.confidence = Some(7.0);
    let mut empty = seg(2);
    empty.text = "   ".into();
    let mut interim = seg(3);
    interim.is_final = false;
    for s in [long, empty, interim] {
        sink.submit(s).await;
    }
    sink.flush().await.unwrap();
    let reqs = srv.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    let segs = reqs[0].2["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0]["text"].as_str().unwrap().len(), 2000);
    assert_eq!(segs[0]["startMs"], 2_147_483_647u64);
    assert_eq!(segs[0]["endMs"], 2_147_483_647u64);
    assert_eq!(segs[0]["confidence"], 1.0);
    assert_eq!(sink.stats().rejected, 2);
    sink.close().await;
}
