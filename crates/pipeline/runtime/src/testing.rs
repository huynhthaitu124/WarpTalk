//! Test doubles and synthetic audio. Public so other pipeline crates can reuse them.

use std::collections::HashMap;
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
