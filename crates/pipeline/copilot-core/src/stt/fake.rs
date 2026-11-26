//! Deterministic STT for tests and offline demos: every committed utterance yields one
//! segment "utterance N on <channel>" (N counts commits per session, from 0; the channel is
//! the session's `speaker`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttError, SttEvent, SttSegment, SttSession,
};

struct Shared {
    latency: Duration,
    sessions_opened: AtomicU64,
    commits: AtomicU64,
    fed_samples: AtomicU64,
    /// Next `open_session` calls that fail.
    fail_opens: AtomicU64,
}

/// Cheap-to-clone handle; clones share counters (keep one for assertions, give another to the
/// engine as `Arc<dyn SpeechToText>`).
#[derive(Clone)]
pub struct FakeStt {
    inner: Arc<Shared>,
}

impl FakeStt {
    pub fn new(latency: Duration) -> Self {
        Self {
            inner: Arc::new(Shared {
                latency,
                sessions_opened: AtomicU64::new(0),
                commits: AtomicU64::new(0),
                fed_samples: AtomicU64::new(0),
                fail_opens: AtomicU64::new(0),
            }),
        }
    }

    pub fn sessions_opened(&self) -> u64 {
        self.inner.sessions_opened.load(Ordering::SeqCst)
    }

    pub fn commits(&self) -> u64 {
        self.inner.commits.load(Ordering::SeqCst)
    }

    pub fn fed_samples(&self) -> u64 {
        self.inner.fed_samples.load(Ordering::SeqCst)
    }

    pub fn fail_next_opens(&self, n: u64) {
        self.inner.fail_opens.store(n, Ordering::SeqCst);
    }
}

struct FakeSession {
    parent: Arc<Shared>,
    channel: String,
    sink: mpsc::Sender<SttEvent>,
    n: u64,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

#[async_trait]
impl SpeechToText for FakeStt {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        if self
            .inner
            .fail_opens
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
            .is_ok()
        {
            return Err(SttError::Connect("fake: open failed".into()));
        }
        self.inner.sessions_opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeSession {
            parent: self.inner.clone(),
            channel: params.speaker,
            sink,
            n: 0,
            tasks: Vec::new(),
        }))
    }
}

#[async_trait]
impl SttSession for FakeSession {
    async fn feed(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        self.parent
            .fed_samples
            .fetch_add(pcm_16k.len() as u64, Ordering::SeqCst);
        Ok(())
    }

    async fn commit(&mut self, _req: CommitRequest) -> Result<(), SttError> {
        let text = format!("utterance {} on {}", self.n, self.channel);
        self.n += 1;
        self.parent.commits.fetch_add(1, Ordering::SeqCst);
        let sink = self.sink.clone();
        let latency = self.parent.latency;
        // Spawned so commit returns immediately (contract); the single task chain keeps order
        // because each utterance's task awaits the previous one.
        let prev = self.tasks.pop();
        self.tasks.push(tokio::spawn(async move {
            if let Some(p) = prev {
                let _ = p.await;
            }
            if !latency.is_zero() {
                tokio::time::sleep(latency).await;
            }
            let _ = sink
                .send(SttEvent::Segment(SttSegment::new(text, "en", None, false)))
                .await;
            let _ = sink.send(SttEvent::UtteranceDone { ok: true }).await;
        }));
        Ok(())
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        Ok(())
    }

    async fn close(mut self: Box<Self>) {
        for t in self.tasks.drain(..) {
            let _ = t.await;
        }
    }
}
