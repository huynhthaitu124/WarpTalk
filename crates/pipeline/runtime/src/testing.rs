//! Test doubles and synthetic audio. Public so other pipeline crates can reuse them.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use warptalk_audio::Frame;
use warptalk_ingress::{EnergyVad, FRAME_SAMPLES, SAMPLE_RATE_HZ, Vad};

use crate::error::SttError;
use crate::messages::{AudioFrame, SttSegment};
use crate::speaker::VadFactory;
use crate::stt::{CommitRequest, SessionParams, SpeechToText, SttEvent, SttSession};

/// 32 ms of 440 Hz sine at `amp` (peak).
pub fn tone_frame(amp: f32, phase_frames: usize) -> AudioFrame {
    let base = phase_frames * FRAME_SAMPLES;
    let samples = (0..FRAME_SAMPLES)
        .map(|i| {
            let t = (base + i) as f32 / SAMPLE_RATE_HZ as f32;
            amp * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect();
    AudioFrame::now(Frame::new(samples, SAMPLE_RATE_HZ))
}

pub fn silence_frame() -> AudioFrame {
    AudioFrame::now(Frame::new(vec![0.0; FRAME_SAMPLES], SAMPLE_RATE_HZ))
}

/// `speech_ms` of tone then `silence_ms` of silence, in 32 ms frames.
pub fn utterance_frames(speech_ms: u32, silence_ms: u32) -> Vec<AudioFrame> {
    let mut v: Vec<AudioFrame> = (0..speech_ms / 32)
        .map(|i| tone_frame(0.3, i as usize))
        .collect();
    v.extend((0..silence_ms / 32).map(|_| silence_frame()));
    v
}

pub fn energy_vad_factory() -> VadFactory {
    Arc::new(|| Box::new(EnergyVad::default()) as Box<dyn Vad + Send>)
}

/// What the fake saw at commit.
#[derive(Debug, Clone)]
pub struct CommitRecord {
    pub speaker: String,
    pub utterance_seq: u64,
    pub hangover_ms: u32,
    pub streamed_samples: usize,
    pub audio_samples: usize,
}

#[derive(Debug, Clone)]
pub struct FakeSttConfig {
    /// Delay from commit to the final segment, per speaker (falls back to `default_delay`).
    pub delay_by_speaker: HashMap<String, Duration>,
    pub default_delay: Duration,
    /// Delay from commit to the early segment (when `early_segment`).
    pub early_delay: Duration,
    pub early_segment: bool,
    /// Sleep inside `feed` (simulates a stalled socket).
    pub feed_delay: Duration,
}

impl Default for FakeSttConfig {
    fn default() -> Self {
        Self {
            delay_by_speaker: HashMap::new(),
            default_delay: Duration::from_millis(10),
            early_delay: Duration::ZERO,
            early_segment: true,
            feed_delay: Duration::ZERO,
        }
    }
}

/// Fake STT. Segment text is `"{speaker}-{utterance_seq}-e"` (early) / `"-f"` (final).
/// Commits of one session are processed strictly in order by a worker task.
#[derive(Debug, Default)]
pub struct FakeStt {
    pub cfg: FakeSttConfig,
    pub commits: Arc<Mutex<Vec<CommitRecord>>>,
    pub fed_samples: Arc<AtomicUsize>,
}

impl FakeStt {
    pub fn new(cfg: FakeSttConfig) -> Self {
        Self {
            cfg,
            ..Self::default()
        }
    }

    pub fn commit_records(&self) -> Vec<CommitRecord> {
        self.commits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

struct FakeSession {
    cfg: FakeSttConfig,
    delay: Duration,
    speaker: String,
    jobs: mpsc::UnboundedSender<CommitRequest>,
    worker: JoinHandle<()>,
    commits: Arc<Mutex<Vec<CommitRecord>>>,
    fed: Arc<AtomicUsize>,
}

#[async_trait]
impl SpeechToText for FakeStt {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        let delay = self
            .cfg
            .delay_by_speaker
            .get(&params.speaker)
            .copied()
            .unwrap_or(self.cfg.default_delay);
        let (jobs, mut rx) = mpsc::unbounded_channel::<CommitRequest>();
        let cfg = self.cfg.clone();
        let worker = tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let u = &req.utterance;
                if cfg.early_segment {
                    tokio::time::sleep(cfg.early_delay).await;
                    let text = format!("{}-{}-e", u.speaker, u.seq);
                    let _ = sink
                        .send(SttEvent::Segment(SttSegment::new(text, "en", None, true)))
                        .await;
                }
                tokio::time::sleep(delay).await;
                let text = format!("{}-{}-f", u.speaker, u.seq);
                let _ = sink
                    .send(SttEvent::Segment(SttSegment::new(
                        text,
                        "en",
                        Some(-0.1),
                        false,
                    )))
                    .await;
                let _ = sink.send(SttEvent::UtteranceDone { ok: true }).await;
            }
        });
        Ok(Box::new(FakeSession {
            cfg: self.cfg.clone(),
            delay,
            speaker: params.speaker,
            jobs,
            worker,
            commits: self.commits.clone(),
            fed: self.fed_samples.clone(),
        }))
    }
}

#[async_trait]
impl SttSession for FakeSession {
    async fn feed(&mut self, pcm: &[f32]) -> Result<(), SttError> {
        if !self.cfg.feed_delay.is_zero() {
            tokio::time::sleep(self.cfg.feed_delay).await;
        }
        self.fed.fetch_add(pcm.len(), Ordering::Relaxed);
        Ok(())
    }

    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError> {
        let _ = self.delay;
        self.commits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(CommitRecord {
                speaker: self.speaker.clone(),
                utterance_seq: req.utterance.seq,
                hangover_ms: req.utterance.hangover_ms,
                streamed_samples: req.streamed_samples,
                audio_samples: req.utterance.audio.len(),
            });
        self.jobs.send(req).map_err(|_| SttError::Closed)
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        Ok(())
    }

    async fn close(self: Box<Self>) {
        let Self { jobs, worker, .. } = *self;
        drop(jobs);
        let _ = worker.await;
    }
}

/// A minimal [`crate::Utterance`] for tests of STT implementations.
pub fn test_utterance(room: &str, speaker: &str, seq: u64, audio: Vec<f32>) -> crate::Utterance {
    let ms = (audio.len() as u64 * 1000 / u64::from(SAMPLE_RATE_HZ)) as u32;
    let mut stamps = crate::Stamps::new(std::time::Instant::now());
    stamps.stt_commit = Some(std::time::Instant::now());
    crate::Utterance {
        room: room.into(),
        speaker: speaker.into(),
        seq,
        lang: None,
        audio: audio.into(),
        speech_ms: ms,
        total_ms: ms,
        hangover_ms: 0,
        reason: warptalk_ingress::EndReason::Silence,
        stamps,
    }
}

// ---------------------------------------------------------------------------------------------
// Jittered streaming STT double
// ---------------------------------------------------------------------------------------------

/// Standard normal (Box-Muller).
fn gaussian(rng: &mut rand::rngs::StdRng) -> f64 {
    use rand::Rng;
    let u1: f64 = rng.gen_range(f64::EPSILON..1.0);
    let u2: f64 = rng.gen_range(0.0..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Log-normal sample with the given median and 95th percentile (per-turn jitter).
pub fn lognormal_ms(rng: &mut rand::rngs::StdRng, p50: f64, p95: f64) -> f64 {
    let sigma = (p95.max(p50) / p50.max(1e-9)).ln() / 1.645;
    p50 * (sigma * gaussian(rng)).exp()
}

/// Model of a streaming STT (spec 08 section 1 / 3.1): audio is transcribed while it arrives,
/// every sentence is published as an early segment `lag` after its audio ended (the live text
/// trails the speaker by 0-0.6 s), the trailing sentence completes `tail_lag` after the last
/// speech frame (the model sees silence), and the turn's `UtteranceDone` follows
/// `commit_to_done` after the commit (spec: paced commit to completed p50 774 ms). Anything
/// not flushed early arrives as the remainder at that point.
#[derive(Debug, Clone)]
pub struct JitterSttConfig {
    /// Streaming transcription (early sentences before commit). `false` = every turn is
    /// transcribed only at commit (closed-chunk path).
    pub early: bool,
    /// Audio per sentence (turns longer than this get several early sentences).
    pub sentence_ms: u32,
    /// Relative +-jitter of `sentence_ms` per sentence.
    pub sentence_jitter: f64,
    /// A tail shorter than this is not a sentence of its own (merged into the remainder path).
    pub min_tail_ms: u32,
    /// Sentence end (audio) to early segment, mid-turn sentences.
    pub lag_p50_ms: f64,
    pub lag_p95_ms: f64,
    /// Last speech frame to the early segment of the trailing sentence.
    pub tail_lag_p50_ms: f64,
    pub tail_lag_p95_ms: f64,
    /// Commit to `UtteranceDone` (and to the remainder when text was not flushed early).
    pub commit_to_done_p50_ms: f64,
    pub commit_to_done_p95_ms: f64,
    pub seed: u64,
}

impl Default for JitterSttConfig {
    fn default() -> Self {
        Self {
            early: true,
            sentence_ms: 2_000,
            sentence_jitter: 0.2,
            min_tail_ms: 300,
            lag_p50_ms: 450.0,
            lag_p95_ms: 800.0,
            tail_lag_p50_ms: 450.0,
            tail_lag_p95_ms: 800.0,
            commit_to_done_p50_ms: 774.0,
            commit_to_done_p95_ms: 1_300.0,
            seed: 7,
        }
    }
}

/// Log of what the jittered STT did, for assertions.
#[derive(Debug, Default)]
pub struct JitterSttLog {
    /// `(speaker, utterance_seq, text, is_early)` in emission order.
    pub emitted: Mutex<Vec<(String, u64, String, bool)>>,
    pub commits: AtomicUsize,
    /// `hangover_ms` of every commit (stream time from last speech frame to the decision).
    pub hangovers: Mutex<Vec<u32>>,
}

impl JitterSttLog {
    pub fn hangovers(&self) -> Vec<u32> {
        self.hangovers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn emitted(&self) -> Vec<(String, u64, String, bool)> {
        self.emitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug)]
pub struct JitterStt {
    pub cfg: JitterSttConfig,
    pub log: Arc<JitterSttLog>,
    sessions: AtomicUsize,
}

impl JitterStt {
    pub fn new(cfg: JitterSttConfig) -> Self {
        Self {
            cfg,
            log: Arc::new(JitterSttLog::default()),
            sessions: AtomicUsize::new(0),
        }
    }
}

enum JCmd {
    Feed {
        samples: usize,
        at: std::time::Instant,
    },
    Commit {
        speech_ms: u32,
        at: std::time::Instant,
        streamed: bool,
    },
    Discard,
}

#[async_trait]
impl SpeechToText for JitterStt {
    async fn open_session(
        &self,
        params: crate::stt::SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        use rand::SeedableRng;
        let n = self.sessions.fetch_add(1, Ordering::Relaxed) as u64;
        let rng = rand::rngs::StdRng::seed_from_u64(self.cfg.seed ^ (n.wrapping_mul(0x9E37_79B9)));
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(jitter_worker(
            self.cfg.clone(),
            self.log.clone(),
            params.speaker,
            rng,
            rx,
            sink,
        ));
        Ok(Box::new(JitterSession {
            tx,
            worker,
            log: self.log.clone(),
        }))
    }
}

struct JitterSession {
    tx: mpsc::UnboundedSender<JCmd>,
    worker: JoinHandle<()>,
    log: Arc<JitterSttLog>,
}

#[async_trait]
impl SttSession for JitterSession {
    async fn feed(&mut self, pcm: &[f32]) -> Result<(), SttError> {
        // Stamp here: the worker may be sleeping on a timer when the command is read.
        self.tx
            .send(JCmd::Feed {
                samples: pcm.len(),
                at: std::time::Instant::now(),
            })
            .map_err(|_| SttError::Closed)
    }

    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError> {
        self.log.commits.fetch_add(1, Ordering::Relaxed);
        self.log
            .hangovers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req.utterance.hangover_ms);
        self.tx
            .send(JCmd::Commit {
                speech_ms: req.utterance.speech_ms,
                at: std::time::Instant::now(),
                streamed: req.streamed_samples > 0,
            })
            .map_err(|_| SttError::Closed)
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        self.tx.send(JCmd::Discard).map_err(|_| SttError::Closed)
    }

    async fn close(self: Box<Self>) {
        let Self { tx, worker, .. } = *self;
        drop(tx);
        let _ = worker.await;
    }
}

struct JTurn {
    fed_samples: u64,
    /// Audio end (samples) of the next sentence boundary.
    next_boundary: u64,
    /// Audio end (samples) of the last sentence scheduled for early emission.
    last_boundary: u64,
    sentences: usize,
    last_feed: std::time::Instant,
    /// When the trailing sentence completes unless more audio arrives.
    tail_due: Option<std::time::Instant>,
    tail_flushed: bool,
    /// Early segments waiting for their due instant, ascending.
    due: VecDeque<(std::time::Instant, String)>,
}

impl JTurn {
    fn new(first_boundary: u64, at: std::time::Instant) -> Self {
        Self {
            fed_samples: 0,
            next_boundary: first_boundary,
            last_boundary: 0,
            sentences: 0,
            last_feed: at,
            tail_due: None,
            tail_flushed: false,
            due: VecDeque::new(),
        }
    }

    fn tail_ms(&self) -> u64 {
        self.fed_samples.saturating_sub(self.last_boundary) / (u64::from(SAMPLE_RATE_HZ) / 1000)
    }
}

fn after(at: std::time::Instant, ms: f64) -> std::time::Instant {
    at + Duration::from_micros((ms.max(0.0) * 1000.0) as u64)
}

async fn jitter_worker(
    cfg: JitterSttConfig,
    log: Arc<JitterSttLog>,
    speaker: String,
    mut rng: rand::rngs::StdRng,
    mut rx: mpsc::UnboundedReceiver<JCmd>,
    sink: mpsc::Sender<SttEvent>,
) {
    use rand::Rng;
    use std::time::Instant;
    let samples_per_ms = u64::from(SAMPLE_RATE_HZ) / 1000;
    let sentence_len = |rng: &mut rand::rngs::StdRng| -> u64 {
        let j = 1.0 + cfg.sentence_jitter * rng.gen_range(-1.0..1.0);
        (f64::from(cfg.sentence_ms) * j).max(500.0) as u64 * samples_per_ms
    };
    let mut utt = 0u64;
    let mut turn: Option<JTurn> = None;
    let record = |text: &str, early: bool, utt: u64| {
        log.emitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((speaker.clone(), utt, text.to_string(), early));
    };
    let sleep_to = |at: Instant| tokio::time::sleep_until(tokio::time::Instant::from_std(at));
    loop {
        let next_due =
            turn.as_ref()
                .and_then(|t| match (t.due.front().map(|(at, _)| *at), t.tail_due) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                });
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    JCmd::Feed { samples, at } => {
                        let first = sentence_len(&mut rng);
                        let t = turn.get_or_insert_with(|| JTurn::new(first, at));
                        if t.tail_flushed {
                            // The speaker went on after the trailing sentence was flushed.
                            t.tail_flushed = false;
                            t.last_boundary = t.fed_samples;
                            t.next_boundary = t.fed_samples + sentence_len(&mut rng);
                        }
                        t.fed_samples += samples as u64;
                        t.last_feed = at;
                        if !cfg.early {
                            continue;
                        }
                        // Audio crossed a sentence boundary: the model completes that sentence
                        // `lag` later. The trailing sentence needs the silence after it.
                        while t.fed_samples >= t.next_boundary {
                            let lag = lognormal_ms(&mut rng, cfg.lag_p50_ms, cfg.lag_p95_ms);
                            let k = t.sentences;
                            t.sentences += 1;
                            t.last_boundary = t.next_boundary;
                            t.next_boundary += sentence_len(&mut rng);
                            t.due.push_back((after(at, lag), format!("Sentence {k} of turn {utt} spoken by the speaker.")));
                        }
                        let tail = lognormal_ms(&mut rng, cfg.tail_lag_p50_ms, cfg.tail_lag_p95_ms);
                        t.tail_due = Some(after(at, tail));
                    }
                    JCmd::Discard => turn = None,
                    JCmd::Commit { speech_ms, at, streamed } => {
                        let mut t = turn.take().unwrap_or_else(|| JTurn::new(0, at));
                        let done_at = after(
                            at,
                            lognormal_ms(&mut rng, cfg.commit_to_done_p50_ms, cfg.commit_to_done_p95_ms),
                        );
                        // Trailing text not flushed early comes with the final transcript.
                        let early_tail = cfg.early && streamed;
                        let has_tail = if early_tail {
                            t.tail_ms() >= u64::from(cfg.min_tail_ms)
                        } else {
                            speech_ms >= cfg.min_tail_ms
                        };
                        let mut remainder = None;
                        if has_tail && !t.tail_flushed {
                            let text = format!("The closing part of turn {utt} said here.");
                            match t.tail_due {
                                Some(due) if early_tail => t.due.push_back((due, text)),
                                _ => remainder = Some(text),
                            }
                        }
                        t.tail_due = None;
                        let mut latest = at;
                        while let Some((due, text)) = t.due.pop_front() {
                            sleep_to(due).await;
                            record(&text, true, utt);
                            latest = latest.max(due);
                            if sink.send(SttEvent::Segment(SttSegment::new(text, "en", None, true))).await.is_err() {
                                return;
                            }
                        }
                        sleep_to(done_at.max(latest)).await;
                        if let Some(text) = remainder {
                            record(&text, false, utt);
                            if sink.send(SttEvent::Segment(SttSegment::new(text, "en", Some(-0.1), false))).await.is_err() {
                                return;
                            }
                        }
                        if sink.send(SttEvent::UtteranceDone { ok: true }).await.is_err() {
                            return;
                        }
                        utt += 1;
                    }
                }
            }
            () = async {
                match next_due {
                    Some(at) => sleep_to(at).await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(t) = turn.as_mut() else { continue };
                let now = Instant::now();
                while t.due.front().is_some_and(|(at, _)| *at <= now) {
                    if let Some((_, text)) = t.due.pop_front() {
                        record(&text, true, utt);
                        if sink.send(SttEvent::Segment(SttSegment::new(text, "en", None, true))).await.is_err() {
                            return;
                        }
                    }
                }
                if t.tail_due.is_some_and(|d| d <= now) && t.due.is_empty() {
                    t.tail_due = None;
                    if t.tail_ms() >= u64::from(cfg.min_tail_ms) {
                        t.tail_flushed = true;
                        let text = format!("The closing part of turn {utt} said here.");
                        record(&text, true, utt);
                        if sink.send(SttEvent::Segment(SttSegment::new(text, "en", None, true))).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}
