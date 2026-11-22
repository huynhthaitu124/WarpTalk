//! One room's realtime pipeline: media session in, `RoomPipeline` (VAD -> STT) in the middle,
//! downstream stage out. No Redis here; ownership and routes come in from the host through
//! [`RoomWorker::set_routes`], so the same worker runs under the benchmark and the tests.
//!
//! Audio path: transport event -> [`FrameChunker`] (resample to 16 kHz, 32 ms frames) ->
//! `SpeakerHandle::push` (synchronous, drop-oldest). The loop never awaits anything slow
//! between an audio event and the push; speaker removal (which drains) runs detached.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use warptalk_audio::{Frame, SAMPLE_RATE_16K, resample};
use warptalk_ingress::FRAME_SAMPLES;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::{
    AudioFrame, ParticipantId, PipelineConfig, PipelineError, RoomDeps, RoomPipeline,
    SpeakerHandle, SpeechToText, VadFactory,
};

use crate::downstream::{DownstreamContext, DownstreamStage};
use crate::results::ResultSink;
use crate::routes::RoutesView;
use crate::transport::{MediaError, MediaEvent, MediaSession, MediaTransport, ParticipantAudio};

/// How long a stopping room waits for its downstream stage to drain before aborting it.
const DOWNSTREAM_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Counts audio blocks of participants that are not (yet) speakers.
pub const AUDIO_IGNORED_METRIC: &str = "warptalk_pipeline_audio_ignored_blocks_total";

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("media: {0}")]
    Media(#[from] MediaError),
    #[error("pipeline: {0}")]
    Pipeline(#[from] PipelineError),
}

pub struct RoomWorkerDeps {
    pub room: RoomId,
    pub transport: Arc<dyn MediaTransport>,
    pub stt: Arc<dyn SpeechToText>,
    pub vad_factory: VadFactory,
    pub downstream: Arc<dyn DownstreamStage>,
    pub results: Arc<dyn ResultSink>,
    /// Base pipeline config; changes are applied through `RoomPipeline::update_config`.
    pub config: watch::Receiver<PipelineConfig>,
    /// Per-room flash mode override (`translationRoom:{id}:flash_mode`), applied over `config`.
    pub flash_override: Option<bool>,
    /// Identity the agent joins under.
    pub identity: String,
    pub initial_routes: RoutesView,
}

/// Cloneable-by-Arc handle of a running room worker.
pub struct RoomWorker {
    room: RoomId,
    routes_tx: watch::Sender<Arc<RoutesView>>,
    stop: watch::Sender<bool>,
    speakers: Arc<Mutex<BTreeSet<ParticipantId>>>,
    finished: Arc<AtomicBool>,
    join: AsyncMutex<Option<JoinHandle<()>>>,
}

impl RoomWorker {
    /// Joins the room and starts processing. Returns once joined.
    pub async fn start(deps: RoomWorkerDeps) -> Result<Self, WorkerError> {
        let room = deps.room;
        let session: Arc<dyn MediaSession> =
            Arc::from(deps.transport.join(room, &deps.identity).await?);
        let events = session.take_events().ok_or(MediaError::Closed)?;
        let base = deps.config.borrow().clone();
        let (pipeline, segments) = RoomPipeline::spawn(
            RoomDeps {
                room: room.to_string(),
                stt: deps.stt,
                vad_factory: deps.vad_factory,
                external_shutdown: None,
            },
            effective_config(&base, deps.flash_override),
        )?;
        let (routes_tx, routes_rx) = watch::channel(Arc::new(deps.initial_routes));
        let (stop_tx, stop_rx) = watch::channel(false);
        let speakers = Arc::new(Mutex::new(BTreeSet::new()));
        let finished = Arc::new(AtomicBool::new(false));

        let downstream = {
            let ctx = DownstreamContext {
                room,
                media: session.clone(),
                results: deps.results,
                routes: routes_rx.clone(),
            };
            let stage = deps.downstream;
            tokio::spawn(async move { stage.run(ctx, segments).await })
        };

        let closing = session.clone();
        let runner = Runner {
            room,
            pipeline,
            events,
            routes_rx,
            stop_rx,
            cfg_rx: deps.config,
            flash_override: deps.flash_override,
            speakers: speakers.clone(),
            active: HashMap::new(),
            identities: HashMap::new(),
        };
        let fin = finished.clone();
        let join = tokio::spawn(async move {
            runner.run().await;
            // Let the downstream stage finish what is in flight (the last translations and the
            // tail of the dub) BEFORE the session closes, bounded so a stuck vendor cannot hold
            // the room open.
            let mut downstream = downstream;
            match tokio::time::timeout(DOWNSTREAM_DRAIN, &mut downstream).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(room = %room, error = %e, "downstream stage task failed"),
                Err(_) => {
                    warn!(room = %room, "downstream stage did not drain in time; aborting it");
                    downstream.abort();
                }
            }
            closing.close().await;
            fin.store(true, Ordering::Release);
        });
        Ok(Self {
            room,
            routes_tx,
            stop: stop_tx,
            speakers,
            finished,
            join: AsyncMutex::new(Some(join)),
        })
    }

    pub fn room(&self) -> RoomId {
        self.room
    }

    /// New routes: speakers are added/removed to match, target languages follow.
    pub fn set_routes(&self, view: RoutesView) {
        let _ = self.routes_tx.send(Arc::new(view));
    }

    pub fn routes(&self) -> Arc<RoutesView> {
        self.routes_tx.borrow().clone()
    }

    /// Participants with a running speaker pipeline.
    pub fn speakers(&self) -> Vec<ParticipantId> {
        self.speakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Graceful stop: speakers drain, the downstream stage finishes, the session closes.
    pub async fn stop(&self) {
        let _ = self.stop.send(true);
        if let Some(j) = self.join.lock().await.take() {
            let _ = j.await;
        }
    }
}

fn effective_config(base: &PipelineConfig, flash: Option<bool>) -> PipelineConfig {
    let mut cfg = base.clone();
    if let Some(f) = flash {
        cfg.flash_mode = f;
    }
    // The downstream stage uses the end-of-turn marker (empty text); transcript-only stages
    // skip it.
    cfg.emit_utterance_end = true;
    cfg
}

struct ActiveSpeaker {
    handle: SpeakerHandle,
    chunker: FrameChunker,
}

struct Runner {
    room: RoomId,
    pipeline: RoomPipeline,
    events: mpsc::Receiver<MediaEvent>,
    routes_rx: watch::Receiver<Arc<RoutesView>>,
    stop_rx: watch::Receiver<bool>,
    cfg_rx: watch::Receiver<PipelineConfig>,
    flash_override: Option<bool>,
    speakers: Arc<Mutex<BTreeSet<ParticipantId>>>,
    active: HashMap<ParticipantId, ActiveSpeaker>,
    /// LiveKit identity -> speaker of the current routes.
    identities: HashMap<String, ParticipantId>,
}

impl Runner {
    async fn run(mut self) {
        info!(room = %self.room, "room worker started");
        let initial = self.routes_rx.borrow_and_update().clone();
        self.reconcile(&initial).await;
        loop {
            tokio::select! {
                biased;
                _ = self.stop_rx.changed() => break,
                ev = self.events.recv() => match ev {
                    Some(MediaEvent::Audio(a)) => self.on_audio(a),
                    Some(MediaEvent::ParticipantJoined { participant, .. }) => {
                        debug!(room = %self.room, %participant, "participant joined");
                    }
                    Some(MediaEvent::ParticipantLeft { participant, .. }) => {
                        debug!(room = %self.room, %participant, "participant left");
                    }
                    None => {
                        warn!(room = %self.room, "media session closed under the worker");
                        break;
                    }
                },
                changed = self.routes_rx.changed() => {
                    if changed.is_err() { break; }
                    let view = self.routes_rx.borrow_and_update().clone();
                    self.reconcile(&view).await;
                }
                changed = self.cfg_rx.changed() => {
                    if changed.is_err() { continue; }
                    let cfg = effective_config(&self.cfg_rx.borrow_and_update(), self.flash_override);
                    if let Err(e) = self.pipeline.update_config(cfg).await {
                        warn!(room = %self.room, error = %e, "pipeline config reload rejected");
                    }
                }
            }
        }
        // Drain every speaker (open utterance committed, transcripts forwarded), then close
        // the output channel, which ends the downstream stage.
        self.pipeline.shutdown().await;
        self.speakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        info!(room = %self.room, "room worker stopped");
    }

    fn on_audio(&mut self, a: ParticipantAudio) {
        // Media is identified by LiveKit identity (a user id); speakers are participants.
        let speaker = self
            .identities
            .get(&a.participant)
            .unwrap_or(&a.participant);
        match self.active.get_mut(speaker) {
            Some(sp) => {
                let handle = &sp.handle;
                sp.chunker.push(&a.frame, a.captured_at, |f| {
                    handle.push(f);
                });
            }
            None => metrics::counter!(AUDIO_IGNORED_METRIC).increment(1),
        }
    }

    async fn reconcile(&mut self, view: &RoutesView) {
        self.identities = view
            .identities
            .iter()
            .map(|(i, p)| (i.clone(), p.clone()))
            .collect();
        let desired = view.active_speakers();
        for (id, info) in &desired {
            if self.active.contains_key(*id) {
                continue;
            }
            match self
                .pipeline
                .add_speaker((*id).clone(), Some(info.source_language.clone()))
                .await
            {
                Ok(handle) => {
                    self.active.insert(
                        (*id).clone(),
                        ActiveSpeaker {
                            handle,
                            chunker: FrameChunker::default(),
                        },
                    );
                    info!(room = %self.room, speaker = %id, lang = %info.source_language, "speaker added");
                }
                Err(e) => warn!(room = %self.room, speaker = %id, error = %e, "add speaker failed"),
            }
        }
        let gone: Vec<ParticipantId> = self
            .active
            .keys()
            .filter(|k| !desired.contains_key(*k))
            .cloned()
            .collect();
        for id in gone {
            self.active.remove(&id);
            info!(room = %self.room, speaker = %id, "speaker removed");
            // Draining can take a while; do not stall the audio loop for the other speakers.
            let pipeline = self.pipeline.clone();
            tokio::spawn(async move {
                let _ = pipeline.remove_speaker(id).await;
            });
        }
        *self.speakers.lock().unwrap_or_else(PoisonError::into_inner) =
            self.active.keys().cloned().collect();
    }
}

/// Converts arbitrary-length blocks at any rate into 32 ms mono 16 kHz frames. Each output
/// frame is stamped with the capture instant of the block that completed it.
#[derive(Debug, Default)]
pub struct FrameChunker {
    buf: Vec<f32>,
}

impl FrameChunker {
    pub fn push(&mut self, frame: &Frame, captured_at: Instant, mut out: impl FnMut(AudioFrame)) {
        if frame.sample_rate_hz == SAMPLE_RATE_16K {
            self.buf.extend_from_slice(&frame.samples);
        } else {
            self.buf.extend(resample(
                &frame.samples,
                frame.sample_rate_hz,
                SAMPLE_RATE_16K,
            ));
        }
        let mut consumed = 0;
        while self.buf.len() - consumed >= FRAME_SAMPLES {
            let chunk = self.buf[consumed..consumed + FRAME_SAMPLES].to_vec();
            consumed += FRAME_SAMPLES;
            out(AudioFrame {
                frame: Frame::new(chunk, SAMPLE_RATE_16K),
                arrived_at: captured_at,
            });
        }
        self.buf.drain(..consumed);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn chunker_48k_10ms_blocks_to_32ms_frames() {
        let mut c = FrameChunker::default();
        let mut frames = 0;
        let now = Instant::now();
        // 1 s of 10 ms blocks at 48 kHz = 31.25 frames of 32 ms.
        for _ in 0..100 {
            c.push(&Frame::new(vec![0.1; 480], 48_000), now, |f| {
                assert_eq!(f.frame.len(), FRAME_SAMPLES);
                assert_eq!(f.frame.sample_rate_hz, 16_000);
                frames += 1;
            });
        }
        assert_eq!(frames, 31);
    }

    #[test]
    fn chunker_16k_passthrough() {
        let mut c = FrameChunker::default();
        let mut frames = 0;
        for _ in 0..4 {
            c.push(&Frame::new(vec![0.0; 320], 16_000), Instant::now(), |_| {
                frames += 1
            });
        }
        assert_eq!(frames, 2); // 1280 samples = 2.5 frames
    }
}
