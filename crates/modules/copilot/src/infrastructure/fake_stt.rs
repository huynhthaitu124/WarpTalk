//! Deterministic server STT for tests and key-less demos (`COPILOT_SERVER_STT=fake`): every
//! committed utterance yields `utterance N on <channel>` (N counts commits per session from 0).
//! With `early_sentences > 0` that many early segments precede the final one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::application::ports::{
    ServerStt, ServerSttError, ServerSttEvent, ServerSttParams, ServerSttSession,
};

#[derive(Default)]
struct Counters {
    sessions: AtomicU64,
    commits: AtomicU64,
    fed_samples: AtomicU64,
    fail_opens: AtomicU64,
}

/// Cheap to clone; clones share counters.
#[derive(Clone, Default)]
pub struct FakeServerStt {
    inner: Arc<Counters>,
    early_sentences: u32,
}

impl FakeServerStt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit this many early segments before each final one.
    pub fn with_early_sentences(mut self, n: u32) -> Self {
        self.early_sentences = n;
        self
    }

    pub fn sessions_opened(&self) -> u64 {
        self.inner.sessions.load(Ordering::SeqCst)
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

struct Session {
    shared: Arc<Counters>,
    params: ServerSttParams,
    events: mpsc::Sender<ServerSttEvent>,
    n: u64,
    early: u32,
    fed: u64,
}

#[async_trait]
impl ServerStt for FakeServerStt {
    async fn open(
        &self,
        params: ServerSttParams,
        events: mpsc::Sender<ServerSttEvent>,
    ) -> Result<Box<dyn ServerSttSession>, ServerSttError> {
        if self
            .inner
            .fail_opens
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
            .is_ok()
        {
            return Err(ServerSttError::Provider("fake: open failed".into()));
        }
        self.inner.sessions.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Session {
            shared: self.inner.clone(),
            params,
            events,
            n: 0,
            early: self.early_sentences,
            fed: 0,
        }))
    }

    fn model(&self) -> String {
        "fake-stt".into()
    }
}

#[async_trait]
impl ServerSttSession for Session {
    async fn feed(&mut self, pcm: &[i16]) -> Result<(), ServerSttError> {
        self.fed += pcm.len() as u64;
        self.shared
            .fed_samples
            .fetch_add(pcm.len() as u64, Ordering::SeqCst);
        Ok(())
    }

    async fn commit(&mut self) -> Result<(), ServerSttError> {
        let lang = self
            .params
            .language
            .clone()
            .or_else(|| self.params.languages.first().cloned());
        let ch = self.params.channel.as_str();
        for i in 0..self.early {
            let _ = self
                .events
                .send(ServerSttEvent::Segment {
                    text: format!("early {i} of utterance {} on {ch}.", self.n),
                    language: lang.clone(),
                    confidence: Some(0.9),
                    early: true,
                })
                .await;
        }
        let _ = self
            .events
            .send(ServerSttEvent::Segment {
                text: format!("utterance {} on {ch}", self.n),
                language: lang,
                confidence: Some(0.9),
                early: false,
            })
            .await;
        let _ = self
            .events
            .send(ServerSttEvent::UtteranceDone { ok: true })
            .await;
        self.n += 1;
        self.fed = 0;
        self.shared.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn discard(&mut self) -> Result<(), ServerSttError> {
        self.fed = 0;
        Ok(())
    }

    async fn close(self: Box<Self>) {}
}
