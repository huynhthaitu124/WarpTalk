//! Test doubles. Public so the TTS crate and the worker app can reuse them.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use warptalk_pipeline_runtime::{Stamps, SttSegment};

use crate::error::MtError;
use crate::prompt::{BatchRequest, TranslateRequest};
use crate::translator::{BatchOutput, TranslateOutput, Translator};

/// Deterministic fake: `"{text}"` becomes `"[{target}] {text}"`; delays and failures per text.
#[derive(Default)]
pub struct FakeTranslator {
    pub delays: Mutex<HashMap<String, Duration>>,
    pub default_delay: Mutex<Duration>,
    pub fail_texts: Mutex<HashSet<String>>,
    pub single_calls: AtomicUsize,
    pub batch_calls: AtomicUsize,
    pub requests: Mutex<Vec<TranslateRequest>>,
    pub batch_requests: Mutex<Vec<BatchRequest>>,
}

impl FakeTranslator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delay_for(&self, text: &str, d: Duration) {
        self.delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(text.to_string(), d);
    }

    pub fn fail_on(&self, text: &str) {
        self.fail_texts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(text.to_string());
    }

    fn delay(&self, text: &str) -> Duration {
        self.delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(text)
            .copied()
            .unwrap_or(
                *self
                    .default_delay
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            )
    }

    fn fails(&self, text: &str) -> bool {
        self.fail_texts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(text)
    }

    pub fn total_calls(&self) -> usize {
        self.single_calls.load(Ordering::SeqCst) + self.batch_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Translator for FakeTranslator {
    async fn translate(&self, req: &TranslateRequest) -> Result<TranslateOutput, MtError> {
        self.single_calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req.clone());
        tokio::time::sleep(self.delay(&req.text)).await;
        if self.fails(&req.text) {
            return Err(MtError::Protocol("scripted failure".into()));
        }
        let now = Instant::now();
        Ok(TranslateOutput {
            text: format!("[{}] {}", req.target_lang, req.text),
            model: "fake".into(),
            first_token_at: now,
            done_at: now,
        })
    }

    async fn translate_batch(&self, req: &BatchRequest) -> Result<BatchOutput, MtError> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        self.batch_requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req.clone());
        let d = req
            .texts
            .iter()
            .map(|t| self.delay(t))
            .max()
            .unwrap_or_default();
        tokio::time::sleep(d).await;
        if req.texts.iter().any(|t| self.fails(t)) {
            return Err(MtError::Protocol("scripted failure".into()));
        }
        Ok(BatchOutput {
            texts: req
                .texts
                .iter()
                .map(|t| format!("[{}] {t}", req.target_lang))
                .collect(),
            model: "fake-batch".into(),
            done_at: Instant::now(),
        })
    }

    async fn polish(&self, text: &str, _language: &str) -> Result<String, MtError> {
        Ok(text.to_string())
    }
}

/// An STT segment for tests.
pub fn segment(room: &str, speaker: &str, seq: u64, text: &str, lang: &str) -> SttSegment {
    let mut s = SttSegment::new(text, lang, Some(-0.1), false);
    s.room = room.into();
    s.speaker = speaker.into();
    s.seq = seq;
    s.utterance_seq = seq;
    s.stamps = Stamps::new(Instant::now());
    s
}
