//! Test doubles. Public so the worker app and other crates can reuse them.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Notify, mpsc};

use crate::error::TtsError;
use crate::ports::{
    CatalogVoice, CloneRequest, ConsentChecker, ContextParams, GenConfig, SpeakStats, SynthContext,
    Synthesizer, VoiceCatalogProvider, VoiceCloner,
};

#[derive(Debug, Clone, Copy)]
pub enum FailKind {
    Transient,
    Permanent,
    /// One chunk is delivered, then a transient failure.
    PartialThenTransient,
}

#[derive(Debug, Clone)]
pub struct SpeakRecord {
    pub text: String,
    pub voice_id: String,
    pub cfg: GenConfig,
}

pub struct FakeScript {
    pub chunks: usize,
    pub chunk_samples: usize,
    pub chunk_delay: Duration,
    /// Voices whose every request fails.
    pub fail_voices: HashMap<String, FailKind>,
    /// Failures consumed one per speak call, in order.
    pub fail_next: VecDeque<FailKind>,
    /// When set, the LAST chunk of each sentence waits for this notify.
    pub hold_last: Option<Arc<Notify>>,
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            chunks: 3,
            chunk_samples: 480,
            chunk_delay: Duration::ZERO,
            fail_voices: HashMap::new(),
            fail_next: VecDeque::new(),
            hold_last: None,
        }
    }
}

/// Fake vendor. "Active" models what the real vendor counts against the plan: a context from
/// its first push until it is closed.
pub struct FakeSynth {
    pub rate: u32,
    pub script: Mutex<FakeScript>,
    pub opened: AtomicUsize,
    pub speaks: Mutex<Vec<SpeakRecord>>,
    pub active: AtomicUsize,
    pub max_active: AtomicUsize,
    pub closed_gracefully: AtomicUsize,
}

impl FakeSynth {
    pub fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            rate,
            script: Mutex::new(FakeScript::default()),
            opened: AtomicUsize::new(0),
            speaks: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            closed_gracefully: AtomicUsize::new(0),
        })
    }

    pub fn with_script(&self, f: impl FnOnce(&mut FakeScript)) {
        f(&mut self.script.lock().unwrap_or_else(PoisonError::into_inner));
    }

    pub fn speak_count(&self) -> usize {
        self.speaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn records(&self) -> Vec<SpeakRecord> {
        self.speaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

struct FakeCtx {
    synth: Arc<FakeSynth>,
    voice_id: String,
    counted: AtomicBool,
    opened_at: Instant,
    spent: bool,
}

impl Drop for FakeCtx {
    fn drop(&mut self) {
        if self.counted.load(Ordering::SeqCst) {
            self.synth.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl Synthesizer for Arc<FakeSynth> {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    async fn open_context(&self, p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeCtx {
            synth: self.clone(),
            voice_id: p.voice_id.clone(),
            counted: AtomicBool::new(false),
            opened_at: Instant::now(),
            spent: false,
        }))
    }
}

fn fail_error(k: FailKind) -> TtsError {
    match k {
        FailKind::Permanent => TtsError::Permanent("scripted refusal".into()),
        _ => TtsError::Transient("scripted drop".into()),
    }
}

#[async_trait]
impl SynthContext for FakeCtx {
    async fn speak(
        &mut self,
        text: &str,
        cfg: &GenConfig,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, TtsError> {
        if !self.counted.swap(true, Ordering::SeqCst) {
            let now = self.synth.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.synth.max_active.fetch_max(now, Ordering::SeqCst);
        }
        self.synth
            .speaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(SpeakRecord {
                text: text.into(),
                voice_id: self.voice_id.clone(),
                cfg: cfg.clone(),
            });
        let (chunks, samples, delay, fail, hold) = {
            let mut s = self
                .synth
                .script
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let fail = s
                .fail_voices
                .get(&self.voice_id)
                .copied()
                .or_else(|| s.fail_next.pop_front());
            (
                s.chunks,
                s.chunk_samples,
                s.chunk_delay,
                fail,
                s.hold_last.clone(),
            )
        };
        let mut first = None;
        let mut total = 0;
        for i in 0..chunks {
            if let Some(FailKind::Transient | FailKind::Permanent) = fail {
                self.spent = true;
                return Err(fail_error(fail.unwrap_or(FailKind::Transient)));
            }
            if let (Some(FailKind::PartialThenTransient), true) = (fail, i == 1) {
                self.spent = true;
                return Err(fail_error(FailKind::Transient));
            }
            if i + 1 == chunks
                && let Some(h) = &hold
            {
                h.notified().await;
            } else if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            first.get_or_insert_with(Instant::now);
            total += samples;
            out.send(vec![0.1; samples])
                .await
                .map_err(|_| TtsError::Cancelled)?;
        }
        Ok(SpeakStats {
            samples: total,
            first_audio_at: first,
        })
    }

    fn is_reusable(&self, idle_expiry: Duration) -> bool {
        !self.spent && self.opened_at.elapsed() <= idle_expiry + Duration::from_secs(3600)
    }

    async fn close(self: Box<Self>) {
        self.synth.closed_gracefully.fetch_add(1, Ordering::SeqCst);
    }

    async fn cancel(self: Box<Self>) {}
}

pub struct StaticConsent(pub bool);

#[async_trait]
impl ConsentChecker for StaticConsent {
    async fn voice_clone_consented(&self, _r: &str, _s: &str) -> Result<bool, TtsError> {
        Ok(self.0)
    }
}

#[derive(Default)]
pub struct FakeCloner {
    pub calls: AtomicUsize,
    pub fail: AtomicBool,
    pub delay: Mutex<Duration>,
    pub deleted: Mutex<Vec<String>>,
}

#[async_trait]
impl VoiceCloner for FakeCloner {
    async fn clone_voice(&self, req: &CloneRequest) -> Result<String, TtsError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let d = *self.delay.lock().unwrap_or_else(PoisonError::into_inner);
        if !d.is_zero() {
            tokio::time::sleep(d).await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(TtsError::Permanent("clip rejected".into()));
        }
        Ok(format!("clone-{}-{n}", req.name))
    }

    async fn delete_voice(&self, id: &str) -> Result<(), TtsError> {
        self.deleted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(id.into());
        Ok(())
    }
}

/// Catalog with a fixed voice list for every language.
pub struct FakeCatalog(pub Vec<String>);

#[async_trait]
impl VoiceCatalogProvider for FakeCatalog {
    async fn voices(&self, _language: &str) -> Vec<CatalogVoice> {
        self.0
            .iter()
            .map(|id| CatalogVoice {
                id: id.clone(),
                name: id.clone(),
            })
            .collect()
    }
    fn default_voice(&self, _language: &str) -> String {
        "default-voice".into()
    }
}
