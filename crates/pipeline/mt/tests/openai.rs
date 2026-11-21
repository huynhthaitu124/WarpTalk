#![allow(clippy::unwrap_used, clippy::result_large_err)]
//! OpenAiTranslator against a mock Realtime server and a fake chat provider. No real network.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{FakeLlm, user_text};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::protocol::Message;
use warptalk_llm::LlmError;
use warptalk_mt::{
    BatchRequest, MtConfig, OpenAiTranslator, RealtimeClient, TranslateRequest, Translator,
};

#[derive(Clone, Copy)]
enum Mode {
    Reply,
    Silent,
    ErrorEvent,
}

struct Server {
    accepted: AtomicUsize,
    requests: Mutex<Vec<Value>>,
    mode: Mutex<Mode>,
}

async fn spawn_server(mode: Mode) -> (String, Arc<Server>) {
    let st = Arc::new(Server {
        accepted: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
        mode: Mutex::new(mode),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = st.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                state.accepted.fetch_add(1, Ordering::SeqCst);
                let mut n = 0;
                while let Some(Ok(Message::Text(t))) = ws.next().await {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    state.requests.lock().unwrap().push(v.clone());
                    let rid = v["response"]["metadata"]["request_id"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    let user = v["response"]["input"][0]["content"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    n += 1;
                    let resp_id = format!("resp_{n}");
                    let mode = *state.mode.lock().unwrap();
                    let events: Vec<Value> = match mode {
                        Mode::Silent => vec![],
                        Mode::ErrorEvent => {
                            vec![json!({"type":"error","error":{"message":"boom"}})]
                        }
                        Mode::Reply => vec![
                            // Stray event of another response must be ignored.
                            json!({"type":"response.output_text.delta","response_id":"other","delta":"XXX"}),
                            json!({"type":"response.created","response":{"id":resp_id,"metadata":{"request_id":rid}}}),
                            json!({"type":"response.output_text.delta","response_id":resp_id,"delta":"Xin "}),
                            json!({"type":"response.output_text.delta","response_id":resp_id,"delta":"chao"}),
                            json!({"type":"response.done","response":{"id":resp_id,"status":"completed"}}),
                        ],
                    };
                    let _ = user;
                    for e in events {
                        if ws.send(Message::Text(e.to_string().into())).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            });
        }
    });
    (format!("ws://{addr}/v1/realtime"), st)
}

fn cfg(url: &str) -> MtConfig {
    let mut c = MtConfig::default();
    c.realtime.api_key = "test-key".into();
    c.realtime.base_url = url.into();
    c.realtime.pool_size = 2;
    c.realtime.timeout_ms = 400;
    c.chat.backoff_ms = 5;
    c
}

fn req(text: &str) -> TranslateRequest {
    TranslateRequest {
        text: text.into(),
        source_lang: "en".into(),
        target_lang: "vi".into(),
        glossary: vec![],
        context: vec![],
        repair: false,
    }
}

fn translator(
    c: MtConfig,
    rt: bool,
    llm: Arc<FakeLlm>,
) -> (
    OpenAiTranslator,
    Option<Arc<RealtimeClient>>,
    watch::Sender<MtConfig>,
) {
    let (tx, rx) = watch::channel(c.clone());
    let client = rt.then(|| RealtimeClient::new(c.realtime.clone()));
    (OpenAiTranslator::new(rx, client.clone(), llm), client, tx)
}

#[tokio::test]
async fn realtime_first_sentence_uses_warm_socket_and_request_shape() {
    let (url, st) = spawn_server(Mode::Reply).await;
    let llm = FakeLlm::new(false, |_, _| Ok("chat".into()));
    let c = cfg(&url);
    let (t, rt, _tx) = translator(c, true, llm.clone());
    assert_eq!(rt.as_ref().unwrap().warm_up().await, 2);
    let t0 = Instant::now();
    let out = t.translate(&req("Hello there.")).await.unwrap();
    assert_eq!(out.text, "Xin chao");
    assert_eq!(out.model, "gpt-realtime-2.1-mini");
    assert!(out.first_token_at <= out.done_at);
    assert!(t0.elapsed() < Duration::from_millis(300));
    assert_eq!(llm.calls(), 0);
    let sent = st.requests.lock().unwrap()[0].clone();
    assert_eq!(sent["type"], "response.create");
    assert_eq!(sent["response"]["conversation"], "none");
    assert_eq!(sent["response"]["max_output_tokens"], 128);
    assert_eq!(sent["response"]["reasoning"]["effort"], "minimal");
    assert_eq!(sent["response"]["output_modalities"], json!(["text"]));
    assert!(
        sent["response"]["instructions"]
            .as_str()
            .unwrap()
            .contains("interpreter")
    );
    assert!(
        sent["response"]["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Hello there.")
    );
    // The socket went back to the pool: a second call opens no new connection after refill.
    t.translate(&req("Again.")).await.unwrap();
    let stats = rt.unwrap().stats();
    assert_eq!(stats.warm_claims, 2);
    assert_eq!(stats.cold_connects, 0);
}

#[tokio::test]
async fn realtime_timeout_falls_back_to_chat() {
    let (url, _st) = spawn_server(Mode::Silent).await;
    let llm = FakeLlm::new(false, |_, _| Ok("Xin chao tu chat".into()));
    let mut c = cfg(&url);
    c.realtime.timeout_ms = 150;
    let (t, rt, _tx) = translator(c, true, llm.clone());
    rt.unwrap().warm_up().await;
    let t0 = Instant::now();
    let out = t.translate(&req("Hello.")).await.unwrap();
    assert_eq!(out.text, "Xin chao tu chat");
    assert_eq!(out.model, "gpt-5.4-nano");
    assert_eq!(llm.calls(), 1);
    // Bounded: realtime bound plus a fast fallback, never the cliff.
    assert!(t0.elapsed() < Duration::from_millis(1000));
    // Deterministic: temperature 0 was requested from the chat model.
    assert_eq!(llm.requests.lock().unwrap()[0].temperature, Some(0.0));
}

#[tokio::test]
async fn realtime_error_event_falls_back_and_unreachable_pool_too() {
    let (url, _st) = spawn_server(Mode::ErrorEvent).await;
    let llm = FakeLlm::new(false, |_, _| Ok("chat ok".into()));
    let (t, _rt, _tx) = translator(cfg(&url), true, llm.clone());
    assert_eq!(t.translate(&req("Hi.")).await.unwrap().text, "chat ok");
    // Nothing listening at all: connect fails, chat still answers.
    let (t2, _rt2, _tx2) = translator(cfg("ws://127.0.0.1:1/v1/realtime"), true, llm.clone());
    assert_eq!(t2.translate(&req("Hi.")).await.unwrap().text, "chat ok");
}

#[tokio::test]
async fn chat_retries_transient_errors_then_gives_up_on_permanent() {
    let llm = FakeLlm::new(false, |_, i| {
        if i < 2 {
            Err(LlmError::Overloaded("busy".into()))
        } else {
            Ok("done".into())
        }
    });
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    assert_eq!(t.translate(&req("Hi.")).await.unwrap().text, "done");
    assert_eq!(llm.calls(), 3);

    let llm = FakeLlm::new(false, |_, _| Err(LlmError::Auth("bad key".into())));
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    assert!(t.translate(&req("Hi.")).await.is_err());
    assert_eq!(llm.calls(), 1, "auth errors are never retried");
}

fn batch(texts: &[&str]) -> BatchRequest {
    BatchRequest {
        texts: texts.iter().map(|s| (*s).into()).collect(),
        source_lang: "en".into(),
        target_lang: "vi".into(),
        glossary: vec![],
        context: vec![],
        repair: false,
    }
}

#[tokio::test]
async fn batch_parses_json_schema_output_in_one_call() {
    let llm = FakeLlm::new(true, |_, _| {
        Ok(r#"{"translations":["mot","hai","ba"]}"#.into())
    });
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    let out = t
        .translate_batch(&batch(&["one", "two", "three"]))
        .await
        .unwrap();
    assert_eq!(out.texts, ["mot", "hai", "ba"]);
    assert_eq!(llm.calls(), 1, "N sentences, ONE call");
    let r = llm.requests.lock().unwrap()[0].clone();
    let fmt = r.response_format.clone().unwrap();
    assert!(fmt.strict);
    assert_eq!(fmt.schema["required"], json!(["translations"]));
    assert_eq!(r.max_output_tokens, Some(512 * 3));
    let u = user_text(&r);
    assert!(u.contains("[1] one") && u.contains("[3] three"));
}

#[tokio::test]
async fn batch_without_schema_support_uses_numbered_lines() {
    let llm = FakeLlm::new(false, |_, _| Ok("[2] hai\n[1] mot".into()));
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    let out = t.translate_batch(&batch(&["one", "two"])).await.unwrap();
    assert_eq!(out.texts, ["mot", "hai"]);
    assert!(llm.requests.lock().unwrap()[0].response_format.is_none());
}

#[tokio::test]
async fn batch_parse_mismatch_falls_back_to_concurrent_per_sentence() {
    // Call 0 is the batch (bad); calls 1..=3 are per-sentence and each takes 100 ms.
    let mut llm = FakeLlm::new(true, |req, i| {
        if i == 0 {
            Ok(r#"{"translations":["only one"]}"#.into())
        } else {
            Ok(format!(
                "T({})",
                user_text(req).lines().nth(2).unwrap_or("")
            ))
        }
    });
    Arc::get_mut(&mut llm).unwrap().delay = Duration::from_millis(100);
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    let t0 = Instant::now();
    let out = t
        .translate_batch(&batch(&["a b", "c d", "e f"]))
        .await
        .unwrap();
    assert_eq!(out.texts.len(), 3);
    assert_eq!(llm.calls(), 4);
    // 100 ms batch + ONE concurrent round of 100 ms, not three sequential ones.
    assert!(
        t0.elapsed() < Duration::from_millis(330),
        "{:?}",
        t0.elapsed()
    );
}

#[tokio::test]
async fn polish_asks_chat_model_with_guardian_instruction() {
    let llm = FakeLlm::new(false, |_, _| Ok("So, we ship Friday.".into()));
    let (t, _, _tx) = translator(cfg("ws://127.0.0.1:1/x"), false, llm.clone());
    let p = t.polish("so um we ship friday", "en").await.unwrap();
    assert_eq!(p, "So, we ship Friday.");
    let r = llm.requests.lock().unwrap()[0].clone();
    assert!(r.system.unwrap().contains("do NOT translate"));
    assert!(r.max_output_tokens.unwrap() <= 512);
}
