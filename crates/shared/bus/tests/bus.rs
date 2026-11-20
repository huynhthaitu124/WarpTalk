#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use tokio::sync::watch;
use warptalk_bus::*;

async fn conn() -> Option<ConnectionManager> {
    match std::env::var("REDIS_URL") {
        Ok(u) => Some(connect(&u).await.unwrap()),
        Err(_) => {
            println!("REDIS_URL unset; skipping");
            None
        }
    }
}

fn uniq(p: &str) -> String {
    format!("test:{p}:{}", uuid::Uuid::new_v4())
}

struct H {
    calls: AtomicUsize,
    mode: Mode,
}
enum Mode {
    Ok,
    Fatal,
    AlwaysRetry,
    RetryOnce,
}

#[async_trait]
impl StreamHandler for H {
    async fn handle(&self, _m: &StreamMessage) -> Result<(), HandlerError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::Ok => Ok(()),
            Mode::Fatal => Err(HandlerError::Fatal("boom".into())),
            Mode::AlwaysRetry => Err(HandlerError::Retry("again".into())),
            Mode::RetryOnce if n == 0 => Err(HandlerError::Retry("again".into())),
            Mode::RetryOnce => Ok(()),
        }
    }
}

fn cfg(stream: &str) -> ConsumerConfig {
    let mut c = ConsumerConfig::new(stream, "g");
    c.block_ms = 100;
    c.claim_idle_ms = 50;
    c.claim_every = Duration::from_millis(100);
    c.max_deliveries = 2;
    c
}

async fn start(
    c: &ConnectionManager,
    cfg: ConsumerConfig,
    h: Arc<H>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = watch::channel(false);
    let j = tokio::spawn(run_consumer(c.clone(), cfg, h, rx));
    tokio::time::sleep(Duration::from_millis(300)).await; // group created
    (tx, j)
}

async fn stop(tx: watch::Sender<bool>, j: tokio::task::JoinHandle<()>) {
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), j)
        .await
        .unwrap()
        .unwrap();
}

async fn pending(c: &mut ConnectionManager, s: &str) -> i64 {
    let r: redis::Value = redis::cmd("XPENDING")
        .arg(s)
        .arg("g")
        .query_async(c)
        .await
        .unwrap();
    match r {
        redis::Value::Array(v) => match v.first() {
            Some(redis::Value::Int(n)) => *n,
            _ => -1,
        },
        _ => -1,
    }
}

async fn dl(c: &mut ConnectionManager, key: &str) -> Vec<HashMap<String, String>> {
    let r: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
        .arg(key)
        .arg("-")
        .arg("+")
        .query_async(c)
        .await
        .unwrap();
    r.ids
        .iter()
        .map(|i| {
            i.map
                .iter()
                .map(|(k, v)| (k.clone(), redis::from_redis_value::<String>(v).unwrap()))
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn ack_on_ok() {
    let Some(mut c) = conn().await else { return };
    let s = uniq("ok");
    let h = Arc::new(H {
        calls: 0.into(),
        mode: Mode::Ok,
    });
    let (tx, j) = start(&c, cfg(&s), h.clone()).await;
    xadd_json(&mut c, &s, &serde_json::json!({"a": 1}), 100)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    assert_eq!(pending(&mut c, &s).await, 0);
    stop(tx, j).await;
}

#[tokio::test]
async fn fatal_goes_to_dead_letter() {
    let Some(mut c) = conn().await else { return };
    let s = uniq("fatal");
    let conf = cfg(&s);
    let dls = conf.dead_letter_stream.clone();
    let h = Arc::new(H {
        calls: 0.into(),
        mode: Mode::Fatal,
    });
    let (tx, j) = start(&c, conf, h).await;
    xadd(&mut c, &s, &[("k", "v".into())], 100).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let d = dl(&mut c, &dls).await;
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["k"], "v");
    assert_eq!(d[0]["original_stream"], s);
    assert_eq!(d[0]["error"], "boom");
    assert!(d[0].contains_key("original_id") && d[0].contains_key("failed_at"));
    assert_eq!(pending(&mut c, &s).await, 0);
    stop(tx, j).await;
}

#[tokio::test]
async fn retry_is_reclaimed_then_acked() {
    let Some(mut c) = conn().await else { return };
    let s = uniq("retry");
    let mut conf = cfg(&s);
    conf.max_deliveries = 5;
    let h = Arc::new(H {
        calls: 0.into(),
        mode: Mode::RetryOnce,
    });
    let (tx, j) = start(&c, conf, h.clone()).await;
    xadd(&mut c, &s, &[("k", "v".into())], 100).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
    assert_eq!(pending(&mut c, &s).await, 0);
    stop(tx, j).await;
}

#[tokio::test]
async fn retry_exhaustion_dead_letters() {
    let Some(mut c) = conn().await else { return };
    let s = uniq("exhaust");
    let conf = cfg(&s);
    let dls = conf.dead_letter_stream.clone();
    let h = Arc::new(H {
        calls: 0.into(),
        mode: Mode::AlwaysRetry,
    });
    let (tx, j) = start(&c, conf, h).await;
    xadd(&mut c, &s, &[("k", "v".into())], 100).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(dl(&mut c, &dls).await.len(), 1);
    assert_eq!(pending(&mut c, &s).await, 0);
    stop(tx, j).await;
}

#[tokio::test]
async fn claim_once_works() {
    let Some(mut c) = conn().await else { return };
    let k = uniq("claim");
    assert!(claim_once(&mut c, &k, 10).await.unwrap());
    assert!(!claim_once(&mut c, &k, 10).await.unwrap());
}

#[tokio::test]
async fn lease_ownership() {
    let Some(mut c) = conn().await else { return };
    let k = uniq("lease");
    assert!(try_acquire_lease(&mut c, &k, "a", 5000).await.unwrap());
    assert!(!try_acquire_lease(&mut c, &k, "b", 5000).await.unwrap());
    assert!(renew_lease(&mut c, &k, "a", 5000).await.unwrap());
    assert!(!renew_lease(&mut c, &k, "b", 5000).await.unwrap());
    assert!(!release_lease(&mut c, &k, "b").await.unwrap());
    assert!(release_lease(&mut c, &k, "a").await.unwrap());
    assert!(try_acquire_lease(&mut c, &k, "b", 5000).await.unwrap());
    release_lease(&mut c, &k, "b").await.unwrap();
}

#[tokio::test]
async fn pubsub_roundtrip() {
    use futures::StreamExt;
    let Ok(url) = std::env::var("REDIS_URL") else {
        println!("REDIS_URL unset; skipping");
        return;
    };
    let mut c = connect(&url).await.unwrap();
    let ch = uniq("ch");
    let sub = subscribe(&url, std::slice::from_ref(&ch)).await.unwrap();
    futures::pin_mut!(sub);
    publish_json(&mut c, &ch, &serde_json::json!({"x": 1}))
        .await
        .unwrap();
    let (got_ch, payload) = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got_ch, ch);
    assert_eq!(payload, r#"{"x":1}"#);
}
