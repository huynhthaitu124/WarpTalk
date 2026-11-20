use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::streams::{StreamAutoClaimReply, StreamId, StreamPendingCountReply, StreamReadReply};
use serde::de::DeserializeOwned;
use tokio::sync::watch;

use crate::BusError;

/// One stream entry delivered to a handler.
#[derive(Debug, Clone)]
pub struct StreamMessage {
    pub stream: String,
    pub id: String,
    pub fields: HashMap<String, String>,
}

impl StreamMessage {
    /// Decode the `data` field as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, BusError> {
        let raw = self
            .fields
            .get("data")
            .ok_or_else(|| BusError::Decode("missing field `data`".into()))?;
        serde_json::from_str(raw).map_err(|e| BusError::Decode(e.to_string()))
    }
}

/// Handler outcome on failure.
#[derive(Debug, Clone)]
pub enum HandlerError {
    /// Leave pending; will be reclaimed after `claim_idle_ms`.
    Retry(String),
    /// Dead-letter immediately.
    Fatal(String),
}

#[async_trait]
pub trait StreamHandler: Send + Sync {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError>;
}

#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    pub stream: String,
    pub group: String,
    pub consumer: String,
    pub batch: usize,
    pub block_ms: usize,
    pub claim_idle_ms: u64,
    pub claim_every: Duration,
    pub max_deliveries: u64,
    pub dead_letter_stream: String,
}

impl ConsumerConfig {
    pub fn new(stream: impl Into<String>, group: impl Into<String>) -> Self {
        let stream = stream.into();
        let group = group.into();
        Self {
            consumer: format!("{group}-{}", uuid::Uuid::new_v4()),
            dead_letter_stream: format!("{stream}:{group}:dead-letter"),
            stream,
            group,
            batch: 10,
            block_ms: 2000,
            claim_idle_ms: 60_000,
            claim_every: Duration::from_secs(30),
            max_deliveries: 5,
        }
    }
}

const DEAD_LETTER_MAXLEN: usize = 10_000;

fn to_message(stream: &str, sid: &StreamId) -> StreamMessage {
    let fields = sid
        .map
        .iter()
        .filter_map(|(k, v)| {
            redis::from_redis_value::<String>(v)
                .ok()
                .map(|s| (k.clone(), s))
        })
        .collect();
    StreamMessage {
        stream: stream.to_string(),
        id: sid.id.clone(),
        fields,
    }
}

async fn ack(conn: &mut ConnectionManager, cfg: &ConsumerConfig, id: &str) -> Result<(), BusError> {
    let _: i64 = redis::cmd("XACK")
        .arg(&cfg.stream)
        .arg(&cfg.group)
        .arg(id)
        .query_async(conn)
        .await?;
    Ok(())
}

async fn dead_letter(
    conn: &mut ConnectionManager,
    cfg: &ConsumerConfig,
    msg: &StreamMessage,
    error: &str,
) -> Result<(), BusError> {
    let mut cmd = redis::cmd("XADD");
    cmd.arg(&cfg.dead_letter_stream)
        .arg("MAXLEN")
        .arg("~")
        .arg(DEAD_LETTER_MAXLEN)
        .arg("*");
    for (k, v) in &msg.fields {
        cmd.arg(k).arg(v);
    }
    let failed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    cmd.arg("original_stream")
        .arg(&msg.stream)
        .arg("original_id")
        .arg(&msg.id)
        .arg("error")
        .arg(error)
        .arg("failed_at")
        .arg(failed_at.to_string());
    let _: String = cmd.query_async(conn).await?;
    ack(conn, cfg, &msg.id).await
}

/// Handle one message and apply the ack / dead-letter policy.
async fn process(
    conn: &mut ConnectionManager,
    cfg: &ConsumerConfig,
    handler: &Arc<dyn StreamHandler>,
    msg: &StreamMessage,
) -> Result<(), BusError> {
    match handler.handle(msg).await {
        Ok(()) => ack(conn, cfg, &msg.id).await,
        Err(HandlerError::Retry(e)) => {
            tracing::warn!(stream = %msg.stream, id = %msg.id, error = %e, "handler retry; leaving pending");
            Ok(())
        }
        Err(HandlerError::Fatal(e)) => {
            tracing::error!(stream = %msg.stream, id = %msg.id, error = %e, "handler fatal; dead-lettering");
            dead_letter(conn, cfg, msg, &e).await
        }
    }
}

async fn create_group(conn: &mut ConnectionManager, cfg: &ConsumerConfig) -> Result<(), BusError> {
    let r: Result<String, redis::RedisError> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&cfg.stream)
        .arg(&cfg.group)
        .arg("$")
        .arg("MKSTREAM")
        .query_async(conn)
        .await;
    match r {
        Ok(_) => Ok(()),
        Err(e) if e.code() == Some("BUSYGROUP") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn read_new(
    conn: &mut ConnectionManager,
    cfg: &ConsumerConfig,
    handler: &Arc<dyn StreamHandler>,
) -> Result<(), BusError> {
    let reply: StreamReadReply = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg(&cfg.group)
        .arg(&cfg.consumer)
        .arg("COUNT")
        .arg(cfg.batch)
        .arg("BLOCK")
        .arg(cfg.block_ms)
        .arg("STREAMS")
        .arg(&cfg.stream)
        .arg(">")
        .query_async(conn)
        .await?;
    for key in &reply.keys {
        for sid in &key.ids {
            let msg = to_message(&cfg.stream, sid);
            process(conn, cfg, handler, &msg).await?;
        }
    }
    Ok(())
}

async fn delivery_count(
    conn: &mut ConnectionManager,
    cfg: &ConsumerConfig,
    id: &str,
) -> Result<u64, BusError> {
    let r: StreamPendingCountReply = redis::cmd("XPENDING")
        .arg(&cfg.stream)
        .arg(&cfg.group)
        .arg(id)
        .arg(id)
        .arg(1)
        .query_async(conn)
        .await?;
    Ok(r.ids.first().map(|p| p.times_delivered as u64).unwrap_or(0))
}

async fn reclaim(
    conn: &mut ConnectionManager,
    cfg: &ConsumerConfig,
    handler: &Arc<dyn StreamHandler>,
) -> Result<(), BusError> {
    let mut start = "0-0".to_string();
    loop {
        let reply: StreamAutoClaimReply = redis::cmd("XAUTOCLAIM")
            .arg(&cfg.stream)
            .arg(&cfg.group)
            .arg(&cfg.consumer)
            .arg(cfg.claim_idle_ms)
            .arg(&start)
            .arg("COUNT")
            .arg(cfg.batch)
            .query_async(conn)
            .await?;
        for sid in &reply.claimed {
            let msg = to_message(&cfg.stream, sid);
            let n = delivery_count(conn, cfg, &msg.id).await?;
            if n >= cfg.max_deliveries {
                tracing::error!(stream = %msg.stream, id = %msg.id, deliveries = n, "max deliveries reached; dead-lettering");
                dead_letter(conn, cfg, &msg, &format!("max deliveries ({n}) exceeded")).await?;
            } else {
                process(conn, cfg, handler, &msg).await?;
            }
        }
        if reply.next_stream_id == "0-0" || reply.next_stream_id.is_empty() {
            return Ok(());
        }
        start = reply.next_stream_id;
    }
}

/// Run a consumer-group loop until `shutdown` becomes true. Never panics;
/// Redis errors back off 2s doubling to 30s.
pub async fn run_consumer(
    mut conn: ConnectionManager,
    cfg: ConsumerConfig,
    handler: Arc<dyn StreamHandler>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_secs(2);
    let max_backoff = Duration::from_secs(30);

    // Ensure the group exists (retry with backoff).
    loop {
        if *shutdown.borrow() {
            return;
        }
        match create_group(&mut conn, &cfg).await {
            Ok(()) => break,
            Err(e) => {
                tracing::error!(stream = %cfg.stream, error = %e, "XGROUP CREATE failed");
                if wait_or_shutdown(&mut shutdown, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
    backoff = Duration::from_secs(2);
    let mut last_claim = tokio::time::Instant::now();
    tracing::info!(stream = %cfg.stream, group = %cfg.group, consumer = %cfg.consumer, "consumer started");

    loop {
        if *shutdown.borrow() {
            break;
        }
        let result: Result<(), BusError> = async {
            if last_claim.elapsed() >= cfg.claim_every {
                last_claim = tokio::time::Instant::now();
                reclaim(&mut conn, &cfg, &handler).await?;
            }
            // Interruptible read so shutdown is prompt.
            tokio::select! {
                r = read_new(&mut conn, &cfg, &handler) => r,
                _ = shutdown.changed() => Ok(()),
            }
        }
        .await;
        match result {
            Ok(()) => backoff = Duration::from_secs(2),
            Err(e) => {
                tracing::error!(stream = %cfg.stream, error = %e, backoff = ?backoff, "consumer redis error");
                if wait_or_shutdown(&mut shutdown, backoff).await {
                    break;
                }
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
    tracing::info!(stream = %cfg.stream, group = %cfg.group, "consumer stopped");
}

/// Sleep for `d`; returns true if shutdown was signalled meanwhile.
async fn wait_or_shutdown(shutdown: &mut watch::Receiver<bool>, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => *shutdown.borrow(),
        _ = shutdown.changed() => *shutdown.borrow(),
    }
}
