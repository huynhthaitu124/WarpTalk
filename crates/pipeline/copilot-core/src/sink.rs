//! Segment delivery: the [`SegmentSink`] trait, an in-memory sink for tests and the
//! [`HttpSegmentSink`] that batches to `POST {base}/api/v1/copilot/sessions/{id}/segments`.
//!
//! HTTP sink guarantees:
//! - **Idempotent**: `clientSegmentId` (uuid v7) is fixed at creation; a retried batch carries
//!   the same ids, so the server can de-duplicate.
//! - **Batching**: up to `batch_max` segments, or whatever accumulated within `batch_window_ms`.
//! - **Retry with backoff** (exponential, jittered) on network errors, 401/403/408/425/429 and
//!   5xx. 409 counts as delivered (duplicate). Other 4xx reject the batch (dropped, counted).
//! - **Offline buffer**: while the server is unreachable segments stay in a bounded buffer
//!   (`max_buffered_segments`, oldest dropped first and counted) and are flushed, in order, as
//!   soon as a request succeeds again. `submit` never blocks or fails.
//! - The bearer can be replaced at any time (`set_bearer`), e.g. after a token refresh.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rand::Rng;
use serde::Serialize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::config::SinkConfig;
use crate::error::CopilotError;
use crate::types::{SegmentRecord, SinkStats};

#[async_trait]
pub trait SegmentSink: Send + Sync {
    /// Queue a segment for delivery. Must not block on the network and must not fail.
    async fn submit(&self, segment: SegmentRecord);
    /// Wait until everything submitted so far is delivered, or the flush timeout elapses
    /// (`Err`; undelivered segments stay buffered).
    async fn flush(&self) -> Result<(), CopilotError>;
    /// Flush (best effort) and stop background work.
    async fn close(&self);
    fn stats(&self) -> SinkStats;
}

/// Collects segments in memory.
#[derive(Default)]
pub struct MemorySink {
    segs: Mutex<Vec<SegmentRecord>>,
}

impl MemorySink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn segments(&self) -> Vec<SegmentRecord> {
        self.segs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl SegmentSink for MemorySink {
    async fn submit(&self, segment: SegmentRecord) {
        self.segs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(segment);
    }

    async fn flush(&self) -> Result<(), CopilotError> {
        Ok(())
    }

    async fn close(&self) {}

    fn stats(&self) -> SinkStats {
        let n = self
            .segs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len() as u64;
        SinkStats {
            submitted: n,
            delivered: n,
            ..SinkStats::default()
        }
    }
}

#[derive(Serialize)]
struct Batch<'a> {
    segments: &'a [SegmentRecord],
}

struct Inner {
    cfg: SinkConfig,
    url: String,
    client: reqwest::Client,
    bearer: Mutex<String>,
    buf: Mutex<VecDeque<SegmentRecord>>,
    notify: Notify,
    flush_requested: AtomicBool,
    stop: AtomicBool,
    submitted: AtomicU64,
    delivered: AtomicU64,
    retries: AtomicU64,
    rejected: AtomicU64,
    dropped_overflow: AtomicU64,
    offline: AtomicBool,
}

enum PostOutcome {
    Delivered,
    Retry(String),
    Rejected(u16),
}

impl Inner {
    fn buf(&self) -> std::sync::MutexGuard<'_, VecDeque<SegmentRecord>> {
        self.buf.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn peek_batch(&self) -> Vec<SegmentRecord> {
        self.buf()
            .iter()
            .take(self.cfg.batch_max)
            .cloned()
            .collect()
    }

    fn remove(&self, batch: &[SegmentRecord]) {
        let ids: std::collections::HashSet<&str> =
            batch.iter().map(|s| s.client_segment_id.as_str()).collect();
        self.buf()
            .retain(|s| !ids.contains(s.client_segment_id.as_str()));
    }

    async fn post(&self, batch: &[SegmentRecord]) -> PostOutcome {
        let bearer = self
            .bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(bearer)
            .json(&Batch { segments: batch })
            .send()
            .await;
        match resp {
            Err(e) => PostOutcome::Retry(e.to_string()),
            Ok(r) => {
                let code = r.status().as_u16();
                match code {
                    200..=299 | 409 => PostOutcome::Delivered,
                    401 | 403 | 408 | 425 | 429 | 500..=599 => {
                        PostOutcome::Retry(format!("http {code}"))
                    }
                    _ => PostOutcome::Rejected(code),
                }
            }
        }
    }

    fn backoff_for(&self, attempt: u32) -> Duration {
        let base =
            self.cfg.retry_initial_ms as f64 * self.cfg.retry_multiplier.powi(attempt as i32);
        let capped = base.min(self.cfg.retry_max_ms as f64);
        let jitter = rand::thread_rng().gen_range(0.8..1.2);
        Duration::from_millis((capped * jitter) as u64)
    }
}

pub struct HttpSegmentSink {
    inner: Arc<Inner>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl HttpSegmentSink {
    /// Spawns the delivery task (needs a tokio runtime).
    pub fn spawn(
        cfg: SinkConfig,
        base_url: &str,
        session_id: &str,
        bearer: impl Into<String>,
    ) -> Result<Arc<Self>, CopilotError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.request_timeout_ms))
            .build()
            .map_err(|e| CopilotError::Sink(e.to_string()))?;
        let url = format!(
            "{}/api/v1/copilot/sessions/{}/segments",
            base_url.trim_end_matches('/'),
            session_id
        );
        let inner = Arc::new(Inner {
            cfg,
            url,
            client,
            bearer: Mutex::new(bearer.into()),
            buf: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            flush_requested: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            submitted: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            dropped_overflow: AtomicU64::new(0),
            offline: AtomicBool::new(false),
        });
        let task = tokio::spawn(run(inner.clone()));
        Ok(Arc::new(Self {
            inner,
            task: Mutex::new(Some(task)),
        }))
    }

    pub fn set_bearer(&self, bearer: impl Into<String>) {
        *self
            .inner
            .bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = bearer.into();
    }
}

async fn run(inner: Arc<Inner>) {
    let mut attempt: u32 = 0;
    loop {
        // Wait for work.
        loop {
            if inner.stop.load(Ordering::Relaxed) {
                return;
            }
            if !inner.buf().is_empty() {
                break;
            }
            inner.notify.notified().await;
        }
        // Batching window.
        let window = Duration::from_millis(inner.cfg.batch_window_ms);
        let t0 = Instant::now();
        while t0.elapsed() < window
            && inner.buf().len() < inner.cfg.batch_max
            && !inner.flush_requested.load(Ordering::Relaxed)
            && !inner.stop.load(Ordering::Relaxed)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Drain, oldest first.
        loop {
            if inner.stop.load(Ordering::Relaxed) {
                return;
            }
            let batch = inner.peek_batch();
            if batch.is_empty() {
                break;
            }
            match inner.post(&batch).await {
                PostOutcome::Delivered => {
                    inner.remove(&batch);
                    inner
                        .delivered
                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                    inner.offline.store(false, Ordering::Relaxed);
                    attempt = 0;
                }
                PostOutcome::Rejected(code) => {
                    warn!(
                        code,
                        n = batch.len(),
                        "segment batch rejected by server; dropped"
                    );
                    inner.remove(&batch);
                    inner
                        .rejected
                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                }
                PostOutcome::Retry(why) => {
                    debug!(%why, attempt, "segment post failed; will retry");
                    inner.offline.store(true, Ordering::Relaxed);
                    inner.retries.fetch_add(1, Ordering::Relaxed);
                    let wait = inner.backoff_for(attempt);
                    attempt = attempt.saturating_add(1);
                    // Sleep in slices so close() is honoured promptly.
                    let t = Instant::now();
                    while t.elapsed() < wait && !inner.stop.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(10).min(wait)).await;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl SegmentSink for HttpSegmentSink {
    async fn submit(&self, segment: SegmentRecord) {
        {
            let mut b = self.inner.buf();
            if b.len() >= self.inner.cfg.max_buffered_segments {
                b.pop_front();
                self.inner.dropped_overflow.fetch_add(1, Ordering::Relaxed);
            }
            b.push_back(segment);
        }
        self.inner.submitted.fetch_add(1, Ordering::Relaxed);
        self.inner.notify.notify_one();
    }

    async fn flush(&self) -> Result<(), CopilotError> {
        self.inner.flush_requested.store(true, Ordering::Relaxed);
        self.inner.notify.notify_one();
        let deadline = Instant::now() + Duration::from_millis(self.inner.cfg.flush_timeout_ms);
        let result = loop {
            let left = self.inner.buf().len();
            if left == 0 {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err(CopilotError::Sink(format!(
                    "flush timed out, {left} segment(s) still buffered"
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        self.inner.flush_requested.store(false, Ordering::Relaxed);
        result
    }

    async fn close(&self) {
        let _ = self.flush().await;
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.notify.notify_one();
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(t) = task {
            let _ = tokio::time::timeout(Duration::from_secs(2), t).await;
        }
    }

    fn stats(&self) -> SinkStats {
        let i = &self.inner;
        SinkStats {
            submitted: i.submitted.load(Ordering::Relaxed),
            delivered: i.delivered.load(Ordering::Relaxed),
            retries: i.retries.load(Ordering::Relaxed),
            rejected: i.rejected.load(Ordering::Relaxed),
            dropped_overflow: i.dropped_overflow.load(Ordering::Relaxed),
            buffered: i.buf().len() as u64,
            offline: i.offline.load(Ordering::Relaxed),
        }
    }
}
