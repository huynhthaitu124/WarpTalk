//! Provider doubles with the latency profile of spec 08 section 1, for the benchmark and for
//! local demos (`PIPELINE_FAKE_PROVIDERS=1`). Latencies are drawn per call from a log-normal
//! with the stated median and p95, from a seeded generator, so runs repeat.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rand::SeedableRng;
use rand::rngs::StdRng;
use tokio::sync::mpsc;
use warptalk_mt::{
    BatchOutput, BatchRequest, MtError, TranslateOutput, TranslateRequest, Translator,
};
use warptalk_pipeline_runtime::testing::lognormal_ms;
use warptalk_tts::{ContextParams, GenConfig, SpeakStats, SynthContext, Synthesizer, TtsError};

#[derive(Debug, Clone)]
pub struct FakeMtConfig {
    /// Realtime first sentence, request to complete sentence (spec 08: warm 655-919 ms, p50 685).
    pub first_p50_ms: f64,
    pub first_p95_ms: f64,
    /// Time the first delta precedes the completed sentence.
    pub first_token_lead_ms: f64,
    /// Batched chat call for the remaining sentences (about 800 ms).
    pub batch_p50_ms: f64,
    pub batch_p95_ms: f64,
    pub seed: u64,
}

impl Default for FakeMtConfig {
    fn default() -> Self {
        Self {
            first_p50_ms: 685.0,
            first_p95_ms: 900.0,
            first_token_lead_ms: 80.0,
            batch_p50_ms: 800.0,
            batch_p95_ms: 1_100.0,
            seed: 11,
        }
    }
}

/// `"{text}"` -> `"[{target}] {text}"` after a jittered delay; remembers every request so tests
/// can assert that nothing is translated twice.
pub struct FakeMt {
    cfg: FakeMtConfig,
    rng: Mutex<StdRng>,
    /// `(target_lang, text)` of every call (single and batched sentences).
    pub requests: Mutex<Vec<(String, String)>>,
}

impl FakeMt {
    pub fn new(cfg: FakeMtConfig) -> Arc<Self> {
        Arc::new(Self {
            rng: Mutex::new(StdRng::seed_from_u64(cfg.seed)),
            cfg,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn draw(&self, p50: f64, p95: f64) -> Duration {
        let ms = lognormal_ms(
            &mut self.rng.lock().unwrap_or_else(PoisonError::into_inner),
            p50,
            p95,
        );
        Duration::from_micros((ms * 1000.0) as u64)
    }

    pub fn requests(&self) -> Vec<(String, String)> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Translator for FakeMt {
    async fn translate(&self, req: &TranslateRequest) -> Result<TranslateOutput, MtError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((req.target_lang.clone(), req.text.clone()));
        let total = self.draw(self.cfg.first_p50_ms, self.cfg.first_p95_ms);
        let lead =
            Duration::from_micros((self.cfg.first_token_lead_ms * 1000.0) as u64).min(total / 4);
        tokio::time::sleep(total - lead).await;
        let first_token_at = Instant::now();
        tokio::time::sleep(lead).await;
        Ok(TranslateOutput {
            text: format!("[{}] {}", req.target_lang, req.text),
            model: "fake-realtime".into(),
            first_token_at,
            done_at: Instant::now(),
        })
    }

    async fn translate_batch(&self, req: &BatchRequest) -> Result<BatchOutput, MtError> {
        {
            let mut r = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
            for t in &req.texts {
                r.push((req.target_lang.clone(), t.clone()));
            }
        }
        tokio::time::sleep(self.draw(self.cfg.batch_p50_ms, self.cfg.batch_p95_ms)).await;
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

#[derive(Debug, Clone)]
pub struct FakeTtsConfig {
    pub sample_rate: u32,
    /// Push to first audio on a warm context (spec 08: p50 180 ms).
    pub warm_p50_ms: f64,
    pub warm_p95_ms: f64,
    /// Extra time a cold context costs (669 - 180 ms), spent inside `open_context`.
    pub cold_extra_ms: f64,
    /// Share of context opens that find no warm socket (pool drained, socket expired).
    pub cold_probability: f64,
    /// Audio per character of text; the dub is delivered in `chunk_ms` pieces.
    pub ms_per_char: f64,
    pub chunk_ms: u32,
    /// Delivery speed relative to real time: the vendor streams a sentence faster than it plays
    /// (playout pacing is the track's job, not the stage's). 1.0 delivers in real time.
    pub realtime_factor: f64,
    pub seed: u64,
}

impl Default for FakeTtsConfig {
    fn default() -> Self {
        Self {
            sample_rate: 24_000,
            warm_p50_ms: 180.0,
            warm_p95_ms: 260.0,
            cold_extra_ms: 490.0,
            cold_probability: 0.2,
            ms_per_char: 50.0,
            chunk_ms: 100,
            realtime_factor: 2.0,
            seed: 13,
        }
    }
}

#[derive(Debug, Default)]
pub struct FakeTtsStats {
    pub opened: AtomicU64,
    pub cold_opens: AtomicU64,
    pub sentences: AtomicU64,
}

pub struct FakeTts {
    cfg: FakeTtsConfig,
    rng: Arc<Mutex<StdRng>>,
    pub stats: Arc<FakeTtsStats>,
}

impl FakeTts {
    pub fn new(cfg: FakeTtsConfig) -> Arc<Self> {
        Arc::new(Self {
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(cfg.seed))),
            cfg,
            stats: Arc::new(FakeTtsStats::default()),
        })
    }
}

struct FakeCtx {
    cfg: FakeTtsConfig,
    rng: Arc<Mutex<StdRng>>,
    stats: Arc<FakeTtsStats>,
    opened_at: Instant,
}

#[async_trait]
impl Synthesizer for FakeTts {
    fn sample_rate(&self) -> u32 {
        self.cfg.sample_rate
    }

    async fn open_context(&self, _p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError> {
        use rand::Rng;
        self.stats.opened.fetch_add(1, Ordering::Relaxed);
        let cold = self
            .rng
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .gen_bool(self.cfg.cold_probability.clamp(0.0, 1.0));
        if cold {
            self.stats.cold_opens.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_micros(
                (self.cfg.cold_extra_ms * 1000.0) as u64,
            ))
            .await;
        }
        Ok(Box::new(FakeCtx {
            cfg: self.cfg.clone(),
            rng: self.rng.clone(),
            stats: self.stats.clone(),
            opened_at: Instant::now(),
        }))
    }
}

#[async_trait]
impl SynthContext for FakeCtx {
    async fn speak(
        &mut self,
        text: &str,
        _cfg: &GenConfig,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, TtsError> {
        self.stats.sentences.fetch_add(1, Ordering::Relaxed);
        let first = {
            let ms = lognormal_ms(
                &mut self.rng.lock().unwrap_or_else(PoisonError::into_inner),
                self.cfg.warm_p50_ms,
                self.cfg.warm_p95_ms,
            );
            Duration::from_micros((ms * 1000.0) as u64)
        };
        tokio::time::sleep(first).await;
        let dub_ms = (text.chars().count() as f64 * self.cfg.ms_per_char) as u32;
        let chunks = dub_ms.div_ceil(self.cfg.chunk_ms).max(1);
        let per = (self.cfg.sample_rate * self.cfg.chunk_ms / 1000) as usize;
        let mut first_at = None;
        let mut samples = 0;
        for i in 0..chunks {
            if i > 0 {
                let ms = f64::from(self.cfg.chunk_ms) / self.cfg.realtime_factor.max(0.1);
                tokio::time::sleep(Duration::from_micros((ms * 1000.0) as u64)).await;
            }
            first_at.get_or_insert_with(Instant::now);
            samples += per;
            out.send(vec![0.1; per])
                .await
                .map_err(|_| TtsError::Cancelled)?;
        }
        Ok(SpeakStats {
            samples,
            first_audio_at: first_at,
        })
    }

    fn is_reusable(&self, idle_expiry: Duration) -> bool {
        self.opened_at.elapsed() <= idle_expiry + Duration::from_secs(3600)
    }

    async fn close(self: Box<Self>) {}

    async fn cancel(self: Box<Self>) {}
}
