#![allow(clippy::unwrap_used, clippy::result_large_err)]
//! Pool, context and REST tests against local mock servers. No real network.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;
use warptalk_cartesia::{CartesiaClient, CartesiaConfig, CartesiaError, GenerationConfig};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct Server {
    accepted: AtomicUsize,
    keys: Mutex<Vec<String>>,
    received: Mutex<Vec<Value>>,
    /// Script: the transcript that makes the server answer with an error.
    fail_on: Mutex<Option<String>>,
    /// Never answer a transcript equal to this (to test the sentence timeout).
    hang_on: Mutex<Option<String>>,
}

fn pcm_b64(samples: &[i16]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    B64.encode(bytes)
}

async fn spawn_server(state: Arc<Server>) -> String {
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
                    let k = req
                        .headers()
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    st.keys.lock().unwrap().push(k);
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await else {
                    return;
                };
                state.accepted.fetch_add(1, Ordering::SeqCst);
                let mut flush = 0u64;
                while let Some(Ok(Message::Text(t))) = ws.next().await {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    state.received.lock().unwrap().push(v.clone());
                    let transcript = v["transcript"].as_str().unwrap_or("").to_string();
                    if v["continue"] == json!(false) {
                        let _ = ws
                            .send(Message::Text(json!({"type":"done"}).to_string().into()))
                            .await;
                        continue;
                    }
                    if state.fail_on.lock().unwrap().as_deref() == Some(transcript.as_str()) {
                        let e = json!({"type":"error","error":"voice not found","status_code":404});
                        let _ = ws.send(Message::Text(e.to_string().into())).await;
                        continue;
                    }
                    if state.hang_on.lock().unwrap().as_deref() == Some(transcript.as_str()) {
                        continue;
                    }
                    // Two chunks, the first one ending mid-sample on purpose.
                    let all: Vec<u8> = [1000i16, 2000, 3000, 4000]
                        .iter()
                        .flat_map(|s| s.to_le_bytes())
                        .collect();
                    for part in [&all[..3], &all[3..]] {
                        let c = json!({"type":"chunk","data":B64.encode(part),"flush_id":flush});
                        ws.send(Message::Text(c.to_string().into())).await.unwrap();
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    let _ = pcm_b64(&[0]);
                    let d = json!({"type":"flush_done","flush_id":flush});
                    ws.send(Message::Text(d.to_string().into())).await.unwrap();
                    flush += 1;
                }
            });
        }
    });
    format!("ws://{addr}/tts/websocket")
}

fn cfg(url: &str, pool: usize) -> CartesiaConfig {
    CartesiaConfig {
        api_key: "test-key".into(),
        ws_url: url.into(),
        pool_size: pool,
        sentence_timeout_ms: 400,
        ..CartesiaConfig::default()
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
    let st = Arc::new(Server::default());
    let url = spawn_server(st.clone()).await;
    let client = CartesiaClient::new(cfg(&url, 2)).unwrap();
    assert_eq!(client.warm_up().await, 2);
    assert_eq!(client.pool_stats().idle, 2);
    let t0 = std::time::Instant::now();
    let ctx = client.open_context("voice-1", "en").await.unwrap();
    assert!(t0.elapsed() < Duration::from_millis(50));
    assert_eq!(client.pool_stats().warm_claims, 1);
    // Background refill tops the pool back up to 2.
    wait_for(|| client.pool_stats().idle == 2).await;
    assert!(st.keys.lock().unwrap().iter().all(|k| k == "test-key"));
    ctx.cancel().await;
}

#[tokio::test]
async fn context_streams_chunks_and_continues_prosody() {
    let st = Arc::new(Server::default());
    let url = spawn_server(st.clone()).await;
    let client = CartesiaClient::new(cfg(&url, 1)).unwrap();
    client.warm_up().await;
    let mut ctx = client.open_context("voice-1", "vi").await.unwrap();
    let (tx, mut rx) = mpsc::channel(8);
    let g = GenerationConfig {
        speed: Some(1.2),
        ..GenerationConfig::default()
    };
    // The first chunk must be readable before speak() returns (streaming).
    let speak = tokio::spawn(async move {
        let s1 = ctx.speak("one", &g, &tx).await.unwrap();
        let s2 = ctx.speak("two", &g, &tx).await.unwrap();
        (ctx, s1, s2)
    });
    let first = rx.recv().await.unwrap();
    assert!(!speak.is_finished());
    assert_eq!(first.len(), 1); // 3 bytes -> one sample, one byte carried
    let (ctx, s1, s2) = speak.await.unwrap();
    assert_eq!(s1.samples, 4);
    assert_eq!(s2.samples, 4);
    assert!(s1.first_audio_at.is_some());
    assert_eq!(ctx.sentences_spoken(), 2);
    assert!(ctx.close().await);
    let recv = st.received.lock().unwrap().clone();
    // Same context id on both pushes, continue + flush, speed forwarded, then the end marker.
    assert_eq!(recv[0]["context_id"], recv[1]["context_id"]);
    assert_eq!(recv[0]["continue"], json!(true));
    assert_eq!(recv[0]["flush"], json!(true));
    let speed = recv[0]["generation_config"]["speed"].as_f64().unwrap();
    assert!((speed - 1.2).abs() < 1e-6);
    assert_eq!(recv[0]["voice"]["id"], json!("voice-1"));
    assert_eq!(recv[2]["continue"], json!(false));
    assert_eq!(recv[2]["transcript"], json!(""));
}

#[tokio::test]
async fn api_error_is_classified_and_context_is_spent() {
    let st = Arc::new(Server::default());
    *st.fail_on.lock().unwrap() = Some("bad".into());
    let url = spawn_server(st.clone()).await;
    let client = CartesiaClient::new(cfg(&url, 1)).unwrap();
    let mut ctx = client.open_context("nope", "en").await.unwrap();
    let (tx, _rx) = mpsc::channel(8);
    let err = ctx
        .speak("bad", &GenerationConfig::default(), &tx)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CartesiaError::Api {
            status: Some(404),
            ..
        }
    ));
    assert!(!err.is_transient());
    assert!(!ctx.is_reusable(Duration::from_secs(5)));
    assert!(matches!(
        ctx.speak("x", &GenerationConfig::default(), &tx).await,
        Err(CartesiaError::Closed)
    ));
}

#[tokio::test]
async fn hung_sentence_times_out_as_transient() {
    let st = Arc::new(Server::default());
    *st.hang_on.lock().unwrap() = Some("stall".into());
    let url = spawn_server(st).await;
    let client = CartesiaClient::new(cfg(&url, 1)).unwrap();
    let mut ctx = client.open_context("v", "en").await.unwrap();
    let (tx, _rx) = mpsc::channel(8);
    let err = ctx
        .speak("stall", &GenerationConfig::default(), &tx)
        .await
        .unwrap_err();
    assert_eq!(err, CartesiaError::Timeout("sentence"));
    assert!(err.is_transient());
}

#[tokio::test]
async fn consumer_gone_is_cancelled_not_vendor_fault() {
    let st = Arc::new(Server::default());
    let url = spawn_server(st).await;
    let client = CartesiaClient::new(cfg(&url, 1)).unwrap();
    let mut ctx = client.open_context("v", "en").await.unwrap();
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let err = ctx
        .speak("hi", &GenerationConfig::default(), &tx)
        .await
        .unwrap_err();
    assert_eq!(err, CartesiaError::Cancelled);
}

#[tokio::test]
async fn missing_key_is_not_configured() {
    let c = CartesiaClient::new(CartesiaConfig::default()).unwrap();
    let r = c.open_context("v", "en").await;
    assert!(matches!(r, Err(CartesiaError::NotConfigured(_))));
}

#[tokio::test]
async fn clone_voice_posts_multipart_and_returns_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/voices/clone"))
        .and(header("x-api-key", "k"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"voice-abc"})))
        .mount(&server)
        .await;
    let mut c = CartesiaConfig::new("k");
    c.http_base_url = server.uri();
    let client = CartesiaClient::new(c).unwrap();
    let id = client
        .clone_voice(b"RIFFxxxx", "speaker-1", "vi")
        .await
        .unwrap();
    assert_eq!(id, "voice-abc");
    let reqs = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&reqs[0].body).to_string();
    assert!(body.contains("name=\"name\"") && body.contains("speaker-1"));
    assert!(body.contains("name=\"clip\"") && body.contains("RIFFxxxx"));
    let ct = reqs[0]
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.starts_with("multipart/form-data; boundary="));
}

#[tokio::test]
async fn clone_failure_4xx_is_permanent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/voices/clone"))
        .respond_with(ResponseTemplate::new(400).set_body_string("clip too short"))
        .mount(&server)
        .await;
    let mut c = CartesiaConfig::new("k");
    c.http_base_url = server.uri();
    let client = CartesiaClient::new(c).unwrap();
    let e = client.clone_voice(b"x", "n", "en").await.unwrap_err();
    assert!(!e.is_transient());
}

#[tokio::test]
async fn catalog_paginates_buckets_by_language_and_caches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/voices"))
        .and(wiremock::matchers::query_param("starting_after", "b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[{"id":"c","name":"C","language":"vi"}],"has_more":false})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[{"id":"a","name":"A","language":"en"},{"id":"b","name":"B","language":"vi-VN"}],
            "has_more":true})))
        .mount(&server)
        .await;
    let mut c = CartesiaConfig::new("k");
    c.http_base_url = server.uri();
    let client = CartesiaClient::new(c).unwrap();
    let vi = client.voices("vi").await;
    assert_eq!(
        vi.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        ["b", "c"]
    );
    let en = client.voices("en-US").await;
    assert_eq!(en.len(), 1);
    // Second language lookup is served from the cache: still only the 2 page requests.
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn catalog_failure_yields_empty_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut c = CartesiaConfig::new("k");
    c.http_base_url = server.uri();
    let client = CartesiaClient::new(c).unwrap();
    assert!(client.voices("en").await.is_empty());
}

#[test]
fn config_defaults_validate_and_debug_hides_key() {
    let c = CartesiaConfig::new("sk-secret");
    c.validate().unwrap();
    assert!(!format!("{c:?}").contains("sk-secret"));
    let c: CartesiaConfig = serde_json::from_str(r#"{"sample_rate":24000}"#).unwrap();
    assert_eq!(c.pool_size, 3);
    assert_eq!(c.model_id, "sonic-3.5");
    let bad: CartesiaConfig = serde_json::from_str(r#"{"sample_rate":12345}"#).unwrap();
    assert!(bad.validate().is_err());
}
