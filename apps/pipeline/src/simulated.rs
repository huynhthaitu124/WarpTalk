//! In-process media transport: participants "speak" synthetic speech or WAV/PCM recordings
//! at real-time pace; whatever the pipeline publishes (dub chunks, caption data) is captured
//! with timestamps. Used by tests, `MEDIA_TRANSPORT=simulated` local runs and the latency
//! benchmark (`pipeline-bench`).

use std::collections::HashMap;
use std::f32::consts::PI;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use warptalk_audio::{Frame, resample};
use warptalk_contracts::media::dub_track_name;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::ParticipantId;

use crate::transport::{
    MEDIA_EVENTS_DROPPED_METRIC, MediaError, MediaEvent, MediaSession, MediaTransport,
    ParticipantAudio, PcmChunk,
};
use crate::wav::{WavError, read_wav_file};

/// Where a simulated participant's audio comes from.
#[derive(Debug, Clone)]
pub enum SimSource {
    /// Speech-like bursts (voiced harmonics with syllabic modulation), `speech_ms` of speech
    /// then `pause_ms` of silence, `utterances` times (`None` = forever).
    Synthetic {
        speech_ms: u32,
        pause_ms: u32,
        utterances: Option<u32>,
        amplitude: f32,
        /// Per-turn variation of the speech duration, +-this fraction (the cycle length stays
        /// fixed, the pause absorbs it); deterministic in the turn number.
        speech_jitter: f32,
    },
    /// A recording, already at its own rate.
    Pcm {
        samples: Arc<[f32]>,
        sample_rate_hz: u32,
        looped: bool,
    },
}

/// Speech duration of turn `k`: `speech_ms` varied by +-`jitter` with a hash of `k`, leaving at
/// least 600 ms of pause in the cycle.
fn turn_speech_ms(speech_ms: u32, cycle: u64, jitter: f32, k: u64) -> u64 {
    if jitter <= 0.0 {
        return u64::from(speech_ms);
    }
    let h = k.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let u = ((h >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0;
    let ms = f64::from(speech_ms) * (1.0 + f64::from(jitter * u));
    (ms as u64).clamp(400, cycle.saturating_sub(600).max(400))
}

impl SimSource {
    /// Renders `n` samples starting at sample index `start` of the output stream at `rate`.
    /// The bool is true once the source is exhausted (the returned samples are then padded
    /// with silence).
    fn render(&self, start: u64, n: usize, rate: u32) -> (Vec<f32>, bool) {
        match self {
            SimSource::Synthetic {
                speech_ms,
                pause_ms,
                utterances,
                amplitude,
                speech_jitter,
            } => {
                let cycle = u64::from(speech_ms + pause_ms).max(1);
                let mut out = Vec::with_capacity(n);
                let mut done = false;
                for i in 0..n as u64 {
                    let idx = start + i;
                    let ms = idx * 1000 / u64::from(rate);
                    let (k, within) = (ms / cycle, ms % cycle);
                    if utterances.is_some_and(|u| k >= u64::from(u)) {
                        done = true;
                        out.push(0.0);
                    } else if within < turn_speech_ms(*speech_ms, cycle, *speech_jitter, k) {
                        let t = idx as f32 / rate as f32;
                        let voiced = (2.0 * PI * 140.0 * t).sin()
                            + 0.5 * (2.0 * PI * 280.0 * t).sin()
                            + 0.25 * (2.0 * PI * 420.0 * t).sin();
                        let syllable = 0.7 + 0.3 * (2.0 * PI * 4.0 * t).sin();
                        out.push(amplitude * syllable * voiced / 1.75);
                    } else {
                        out.push(0.0);
                    }
                }
                (out, done)
            }
            SimSource::Pcm {
                samples,
                sample_rate_hz,
                looped,
            } => {
                // Pcm sources are pre-resampled to `rate` by `SimParticipant::new`.
                debug_assert_eq!(*sample_rate_hz, rate);
                let len = samples.len().max(1) as u64;
                let mut out = Vec::with_capacity(n);
                let mut done = false;
                for i in 0..n as u64 {
                    let idx = start + i;
                    if !*looped && idx >= len {
                        done = true;
                        out.push(0.0);
                    } else {
                        out.push(samples.get((idx % len) as usize).copied().unwrap_or(0.0));
                    }
                }
                (out, done)
            }
        }
    }
}

/// One simulated participant.
#[derive(Debug, Clone)]
pub struct SimParticipant {
    pub id: ParticipantId,
    pub source: SimSource,
    /// Delay between joining the room and the first audio.
    pub join_after: Duration,
    /// Leave after this long of playing, if set.
    pub leave_after: Option<Duration>,
    /// Rate of the blocks delivered to the pipeline (48 kHz mimics LiveKit native).
    pub sample_rate_hz: u32,
    pub frame_ms: u32,
}

impl SimParticipant {
    pub fn synthetic(id: impl Into<ParticipantId>, speech_ms: u32, pause_ms: u32) -> Self {
        Self {
            id: id.into(),
            source: SimSource::Synthetic {
                speech_ms,
                pause_ms,
                utterances: None,
                amplitude: 0.3,
                speech_jitter: 0.0,
            },
            join_after: Duration::ZERO,
            leave_after: None,
            sample_rate_hz: 48_000,
            frame_ms: 20,
        }
    }

    pub fn from_wav(
        id: impl Into<ParticipantId>,
        path: &Path,
        looped: bool,
    ) -> Result<Self, WavError> {
        let wav = read_wav_file(path)?;
        Ok(Self::from_pcm(id, wav.samples, wav.sample_rate_hz, looped))
    }

    pub fn from_pcm(
        id: impl Into<ParticipantId>,
        samples: Vec<f32>,
        sample_rate_hz: u32,
        looped: bool,
    ) -> Self {
        let out_rate = 48_000;
        let samples = resample(&samples, sample_rate_hz, out_rate);
        Self {
            id: id.into(),
            source: SimSource::Pcm {
                samples: samples.into(),
                sample_rate_hz: out_rate,
                looped,
            },
            join_after: Duration::ZERO,
            leave_after: None,
            sample_rate_hz: out_rate,
            frame_ms: 20,
        }
    }

    pub fn join_after(mut self, d: Duration) -> Self {
        self.join_after = d;
        self
    }

    pub fn leave_after(mut self, d: Duration) -> Self {
        self.leave_after = Some(d);
        self
    }

    /// Per-turn speech duration jitter of a synthetic source (+- fraction).
    pub fn speech_jitter(mut self, jitter: f32) -> Self {
        if let SimSource::Synthetic { speech_jitter, .. } = &mut self.source {
            *speech_jitter = jitter.clamp(0.0, 0.9);
        }
        self
    }

    pub fn utterances(mut self, n: u32) -> Self {
        if let SimSource::Synthetic { utterances, .. } = &mut self.source {
            *utterances = Some(n);
        }
        self
    }

    /// Delivery format. A `Pcm` source is re-rendered at the new rate.
    pub fn format(mut self, sample_rate_hz: u32, frame_ms: u32) -> Self {
        if let SimSource::Pcm {
            samples,
            sample_rate_hz: src_rate,
            looped,
        } = &self.source
        {
            let resampled = resample(samples, *src_rate, sample_rate_hz);
            self.source = SimSource::Pcm {
                samples: resampled.into(),
                sample_rate_hz,
                looped: *looped,
            };
        }
        self.sample_rate_hz = sample_rate_hz;
        self.frame_ms = frame_ms.max(1);
        self
    }
}

/// Decides who is in a room when the pipeline joins it.
pub type ParticipantPlan = Arc<dyn Fn(&RoomId) -> Vec<SimParticipant> + Send + Sync>;

/// A dub chunk the pipeline published.
#[derive(Debug, Clone)]
pub struct DubRecord {
    /// LiveKit track the chunk went to (`warptalk_contracts::media::dub_track_name`).
    pub track_name: String,
    pub target_lang: String,
    pub voice: String,
    pub samples: usize,
    pub sample_rate_hz: u32,
    pub at: Instant,
}

/// A data message the pipeline published.
#[derive(Debug, Clone)]
pub struct DataRecord {
    pub topic: String,
    pub payload: Vec<u8>,
    pub at: Instant,
}

/// Everything one simulated session published.
#[derive(Debug, Default)]
pub struct SimCapture {
    dubs: Mutex<Vec<DubRecord>>,
    data: Mutex<Vec<DataRecord>>,
    closed: AtomicBool,
    identity: Mutex<String>,
}

impl SimCapture {
    pub fn dubs(&self) -> Vec<DubRecord> {
        self.dubs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn data(&self) -> Vec<DataRecord> {
        self.data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Data payloads of `topic` parsed as JSON.
    pub fn json_on(&self, topic: &str) -> Vec<(Instant, serde_json::Value)> {
        self.data()
            .into_iter()
            .filter(|d| d.topic == topic)
            .filter_map(|d| Some((d.at, serde_json::from_slice(&d.payload).ok()?)))
            .collect()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn identity(&self) -> String {
        self.identity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// See the module docs.
pub struct SimulatedTransport {
    plan: ParticipantPlan,
    captures: Mutex<HashMap<RoomId, Arc<SimCapture>>>,
    /// Capacity of the per-session event channel.
    event_capacity: usize,
}

impl SimulatedTransport {
    pub fn new(plan: ParticipantPlan) -> Self {
        Self {
            plan,
            captures: Mutex::new(HashMap::new()),
            event_capacity: 4096,
        }
    }

    /// Same participants in every room.
    pub fn with_participants(participants: Vec<SimParticipant>) -> Self {
        Self::new(Arc::new(move |_| participants.clone()))
    }

    pub fn with_event_capacity(mut self, cap: usize) -> Self {
        self.event_capacity = cap.max(1);
        self
    }

    /// Capture of the most recent session joined to `room`.
    pub fn capture(&self, room: RoomId) -> Option<Arc<SimCapture>> {
        self.captures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&room)
            .cloned()
    }
}

#[async_trait]
impl MediaTransport for SimulatedTransport {
    async fn join(
        &self,
        room: RoomId,
        identity: &str,
    ) -> Result<Box<dyn MediaSession>, MediaError> {
        let capture = Arc::new(SimCapture::default());
        *capture
            .identity
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = identity.to_owned();
        self.captures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(room, capture.clone());
        let (tx, rx) = mpsc::channel(self.event_capacity);
        let tasks = (self.plan)(&room)
            .into_iter()
            .map(|p| tokio::spawn(play(p, tx.clone())))
            .collect();
        Ok(Box::new(SimulatedSession {
            events: Mutex::new(Some(rx)),
            tx: Mutex::new(Some(tx)),
            tasks: Mutex::new(tasks),
            capture,
        }))
    }
}

fn send_event(tx: &mpsc::Sender<MediaEvent>, ev: MediaEvent) -> bool {
    match tx.try_send(ev) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            metrics::counter!(MEDIA_EVENTS_DROPPED_METRIC).increment(1);
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

/// Plays one participant in real time: frame `i` is due at `start + i * frame_ms`.
async fn play(p: SimParticipant, tx: mpsc::Sender<MediaEvent>) {
    if !send_event(
        &tx,
        MediaEvent::ParticipantJoined {
            participant: p.id.clone(),
            at: Instant::now(),
        },
    ) {
        return;
    }
    tokio::time::sleep(p.join_after).await;
    let frame_samples =
        (u64::from(p.sample_rate_hz) * u64::from(p.frame_ms) / 1000).max(1) as usize;
    let frame_dur = Duration::from_millis(u64::from(p.frame_ms));
    let start = tokio::time::Instant::now();
    let mut idx: u64 = 0;
    loop {
        let due = start + frame_dur * idx as u32;
        tokio::time::sleep_until(due).await;
        if p.leave_after.is_some_and(|l| due - start >= l) {
            break;
        }
        let (samples, done) =
            p.source
                .render(idx * frame_samples as u64, frame_samples, p.sample_rate_hz);
        if done {
            break;
        }
        let ev = MediaEvent::Audio(ParticipantAudio {
            participant: p.id.clone(),
            frame: Frame::new(samples, p.sample_rate_hz),
            captured_at: Instant::now(),
        });
        if !send_event(&tx, ev) {
            return;
        }
        idx += 1;
    }
    send_event(
        &tx,
        MediaEvent::ParticipantLeft {
            participant: p.id,
            at: Instant::now(),
        },
    );
}

struct SimulatedSession {
    events: Mutex<Option<mpsc::Receiver<MediaEvent>>>,
    /// Kept so the event channel stays open after all participants finished.
    tx: Mutex<Option<mpsc::Sender<MediaEvent>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    capture: Arc<SimCapture>,
}

#[async_trait]
impl MediaSession for SimulatedSession {
    fn take_events(&self) -> Option<mpsc::Receiver<MediaEvent>> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    async fn publish_dub(
        &self,
        target_lang: &str,
        voice: &str,
        chunk: PcmChunk,
    ) -> Result<(), MediaError> {
        if self.capture.is_closed() {
            return Err(MediaError::Closed);
        }
        self.capture
            .dubs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(DubRecord {
                track_name: dub_track_name(target_lang),
                target_lang: target_lang.to_owned(),
                voice: voice.to_owned(),
                samples: chunk.samples.len(),
                sample_rate_hz: chunk.sample_rate_hz,
                at: Instant::now(),
            });
        Ok(())
    }

    async fn publish_data(&self, topic: &str, payload: Vec<u8>) -> Result<(), MediaError> {
        if self.capture.is_closed() {
            return Err(MediaError::Closed);
        }
        self.capture
            .data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(DataRecord {
                topic: topic.to_owned(),
                payload,
                at: Instant::now(),
            });
        Ok(())
    }

    async fn close(&self) {
        self.capture.closed.store(true, Ordering::Release);
        self.tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        for t in self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            t.abort();
        }
    }
}

impl Drop for SimulatedSession {
    fn drop(&mut self) {
        for t in self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            t.abort();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_has_speech_then_silence() {
        let src = SimParticipant::synthetic("a", 1000, 1000).source;
        let (speech, _) = src.render(0, 4800, 48_000);
        let (pause, _) = src.render(48_000 + 100, 4800, 48_000);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(&speech) > 0.05, "rms {}", rms(&speech));
        assert_eq!(rms(&pause), 0.0);
    }

    #[test]
    fn finite_synthetic_ends() {
        let p = SimParticipant::synthetic("a", 100, 100).utterances(1);
        let (_, done) = p.source.render(0, 480, 48_000);
        assert!(!done);
        let (_, done) = p.source.render(48_000, 480, 48_000);
        assert!(done);
    }

    #[tokio::test]
    async fn paced_in_real_time_and_captures_publishes() {
        let room = RoomId::new();
        let t = SimulatedTransport::with_participants(vec![
            SimParticipant::synthetic("p1", 500, 500).format(16_000, 20),
        ]);
        let session = t.join(room, "agent").await.unwrap();
        let mut rx = session.take_events().unwrap();
        assert!(session.take_events().is_none());
        let started = Instant::now();
        let mut audio = 0;
        let mut joined = false;
        while started.elapsed() < Duration::from_millis(400) {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Some(MediaEvent::Audio(a))) => {
                    assert_eq!(a.participant, "p1");
                    assert_eq!(a.frame.len(), 320);
                    audio += 1;
                }
                Ok(Some(MediaEvent::ParticipantJoined { .. })) => joined = true,
                Ok(Some(_)) | Ok(None) | Err(_) => {}
            }
        }
        assert!(joined);
        // 400 ms at 20 ms per frame: about 20 blocks, never a burst.
        assert!((15..=22).contains(&audio), "audio blocks: {audio}");

        session
            .publish_dub(
                "vi",
                "v1",
                PcmChunk {
                    samples: vec![0.0; 480].into(),
                    sample_rate_hz: 24_000,
                },
            )
            .await
            .unwrap();
        session
            .publish_data("captions", b"{\"a\":1}".to_vec())
            .await
            .unwrap();
        let cap = t.capture(room).unwrap();
        assert_eq!(cap.identity(), "agent");
        assert_eq!(cap.dubs().len(), 1);
        assert_eq!(cap.dubs()[0].target_lang, "vi");
        assert_eq!(cap.json_on("captions").len(), 1);
        session.close().await;
        assert!(cap.is_closed());
        assert!(session.publish_data("captions", vec![]).await.is_err());
    }
}
