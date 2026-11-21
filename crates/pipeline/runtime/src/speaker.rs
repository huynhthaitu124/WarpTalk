//! One tokio task per speaker: frames in, ordered [`SttSegment`]s out.
//!
//! ```text
//! SpeakerHandle::push_frame (sync, never blocks)
//!        |  FrameQueue (drop-oldest)
//!        v
//!   worker task: VAD -> Endpointer -> LiveFrame -> SttSession::feed   (flash)
//!                                  -> UtteranceEnd -> SttSession::commit
//!        STT sink (bounded mpsc<SttEvent>)
//!        v
//!   relay task: stamps, seq, metrics -> room output channel (bounded)
//! ```
//!
//! Why two tasks: `commit` only *issues* the commit. While turn N is still being transcribed the
//! worker keeps endpointing and streaming turn N+1, so a quick follow-up sentence does not wait
//! for the previous transcript. The relay restores strict order: it is the only writer of the
//! output channel for this speaker and numbers segments in the order the (single) session
//! yields them. Commits cannot interleave because only the worker issues them, sequentially.

use std::collections::VecDeque;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use warptalk_ingress::{DropReason, EndReason, EndpointEvent, Endpointer, Vad};
use warptalk_telemetry::{Outcome, record_stage_latency, record_stage_outcome};

use crate::config::PipelineConfig;
use crate::error::SttError;
use crate::frame_queue::{FrameQueue, PushOutcome};
use crate::messages::{AudioFrame, ParticipantId, RoomId, Stamps, SttSegment, Utterance};
use crate::stt::{CommitRequest, SessionParams, SpeechToText, SttEvent, SttSession};

pub const STAGE_ENDPOINT_DECISION: &str = "endpoint_decision";
pub const STAGE_STT_FIRST_TEXT: &str = "stt_first_text";
pub const STAGE_STT_FINAL: &str = "stt_final";
const STAGE_STT: &str = "stt";

/// Builds one VAD per speaker (Silero is stateful; the energy VAD is not).
pub type VadFactory = Arc<dyn Fn() -> Box<dyn Vad + Send> + Send + Sync>;

/// Frames kept while idle so a config reload can rebuild the endpointer without clipping the
/// pre-speech padding (2 s = the maximum `vad_pre_speech_ms`).
const REPLAY_FRAMES: usize = 64;

#[derive(Debug, Clone)]
pub struct SpeakerSpec {
    pub room: RoomId,
    pub id: ParticipantId,
    pub lang: Option<String>,
}

/// Cheap, cloneable audio entry point of one speaker.
#[derive(Debug, Clone)]
pub struct SpeakerHandle {
    queue: Arc<FrameQueue>,
}

impl SpeakerHandle {
    /// Hands a frame to the speaker task. Synchronous and non-blocking: on overflow the oldest
    /// queued frame is dropped (and counted), never the caller delayed.
    pub fn push(&self, frame: AudioFrame) -> PushOutcome {
        self.queue.push(frame)
    }

    /// Frames evicted by overflow so far.
    pub fn dropped_frames(&self) -> u64 {
        self.queue.dropped()
    }

    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }
}

pub struct SpeakerPipeline {
    handle: SpeakerHandle,
    join: JoinHandle<()>,
}

impl SpeakerPipeline {
    pub fn spawn(
        spec: SpeakerSpec,
        stt: Arc<dyn SpeechToText>,
        vad_factory: &VadFactory,
        cfg_rx: watch::Receiver<Arc<PipelineConfig>>,
        out: mpsc::Sender<SttSegment>,
    ) -> Self {
        let cfg = cfg_rx.borrow().clone();
        let queue = Arc::new(FrameQueue::new(cfg.frame_queue_capacity));
        let tracker = Arc::new(TurnTracker::new());
        let (sink_tx, sink_rx) = mpsc::channel(cfg.stt_event_capacity);
        let relay = tokio::spawn(relay(
            sink_rx,
            tracker.clone(),
            out,
            spec.room.clone(),
            spec.id.clone(),
        ));
        let mut worker = Worker {
            endpointer: Endpointer::new(cfg.endpointer.clone()),
            vad: vad_factory(),
            cfg,
            cfg_rx,
            spec,
            stt,
            sink_tx,
            relay,
            tracker,
            slot: Slot::Empty,
            ring: VecDeque::new(),
            turn_start: None,
            last_speech_arrival: Instant::now(),
            last_arrival: Instant::now(),
            streamed: 0,
            stream_broken: false,
        };
        if worker.cfg.eager_session_open {
            worker.slot = Slot::Opening(worker.open());
        }
        let q = queue.clone();
        let join = tokio::spawn(async move {
            while let Some(frame) = q.pop().await {
                worker.on_frame(frame).await;
            }
            worker.finish().await;
        });
        Self {
            handle: SpeakerHandle { queue },
            join,
        }
    }

    pub fn handle(&self) -> SpeakerHandle {
        self.handle.clone()
    }

    /// Stops input without waiting: queued frames are still processed.
    pub fn close_input(&self) {
        self.handle.queue.close();
    }

    /// Closes input, then waits until every queued frame is processed, the open utterance is
    /// committed and all transcripts have been forwarded (bounded by `shutdown_grace_ms`).
    pub async fn close(self) {
        self.handle.queue.close();
        let _ = self.join.await;
    }

    /// Waits for the task after `close_input`.
    pub fn into_join(self) -> JoinHandle<()> {
        self.join
    }
}

// ---------------------------------------------------------------------------------------------
// Turn tracking shared by worker (writes) and relay (reads)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Turn {
    seq: u64,
    stamps: Stamps,
    first_recorded: bool,
    final_recorded: bool,
}

impl Turn {
    fn fresh(seq: u64, at: Instant) -> Self {
        Self {
            seq,
            stamps: Stamps::new(at),
            first_recorded: false,
            final_recorded: false,
        }
    }
}

#[derive(Debug)]
struct TrackerInner {
    /// The turn being spoken (not yet committed).
    current: Turn,
    /// Committed turns still being transcribed, oldest first.
    committed: VecDeque<Turn>,
}

/// Segments belong to the oldest committed turn still open, else to the turn being spoken
/// (early sentences arrive before commit). Valid because the session yields turns in order.
#[derive(Debug)]
struct TurnTracker {
    inner: Mutex<TrackerInner>,
    /// Early-endpoint hint: the streaming STT produced a sentence terminator for the turn that
    /// is still being spoken. Set by the relay, consumed by the worker (one atomic, no await).
    sentence_hint: AtomicBool,
}

/// Does the text end a sentence (terminator, optionally followed by closing quotes/brackets)?
/// Pure; covers Latin, CJK and Devanagari terminators plus the ellipsis character.
pub(crate) fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['"', '\'', ')', ']', '\u{201d}', '\u{2019}', '\u{bb}', '\u{300d}'])
        .ends_with(['.', '!', '?', '\u{2026}', '\u{3002}', '\u{ff01}', '\u{ff1f}', '\u{0964}'])
}

struct SegmentInfo {
    /// The segment belongs to the turn still being spoken (nothing committed ahead of it).
    uncommitted: bool,
    utterance_seq: u64,
    stamps: Stamps,
    record_first: bool,
    record_final: bool,
}

impl TurnTracker {
    fn new() -> Self {
        Self {
            inner: Mutex::new(TrackerInner {
                current: Turn::fresh(0, Instant::now()),
                committed: VecDeque::new(),
            }),
            sentence_hint: AtomicBool::new(false),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, TrackerInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn set_sentence_hint(&self) {
        self.sentence_hint.store(true, Ordering::Release);
    }
    fn clear_sentence_hint(&self) {
        self.sentence_hint.store(false, Ordering::Release);
    }
    fn sentence_hint_pending(&self) -> bool {
        self.sentence_hint.load(Ordering::Acquire)
    }
    fn start_turn(&self, arrived: Instant) {
        self.clear_sentence_hint();
        let mut g = self.lock();
        let seq = g.current.seq;
        g.current = Turn::fresh(seq, arrived);
    }
    fn seq(&self) -> u64 {
        self.lock().current.seq
    }
    fn commit(&self, speech_end: Instant, commit_at: Instant) {
        self.clear_sentence_hint();
        let mut g = self.lock();
        let mut t = g.current;
        t.stamps.speech_end = Some(speech_end);
        t.stamps.stt_commit = Some(commit_at);
        g.committed.push_back(t);
        g.current = Turn::fresh(t.seq + 1, commit_at);
    }
    fn abort_last_commit(&self) {
        let mut g = self.lock();
        if let Some(t) = g.committed.pop_back() {
            g.current = Turn::fresh(t.seq, t.stamps.audio_arrived);
        }
    }
    fn discard_current(&self) {
        self.clear_sentence_hint();
        let mut g = self.lock();
        let seq = g.current.seq;
        g.current = Turn::fresh(seq, Instant::now());
    }
    fn on_segment(&self, is_final: bool, now: Instant) -> SegmentInfo {
        let mut g = self.lock();
        let uncommitted = g.committed.is_empty();
        let t = match g.committed.front_mut() {
            Some(t) => t,
            None => &mut g.current,
        };
        let mut info = SegmentInfo {
            uncommitted,
            utterance_seq: t.seq,
            stamps: t.stamps,
            record_first: false,
            record_final: false,
        };
        if !t.first_recorded {
            t.first_recorded = true;
            t.stamps.stt_first_text = Some(now);
            info.record_first = true;
        }
        if is_final && !t.final_recorded {
            t.final_recorded = true;
            t.stamps.stt_final = Some(now);
            info.record_final = true;
        }
        info.stamps = t.stamps;
        info
    }
    /// Pops the finished turn; returns its commit stamp when `stt_final` was not recorded yet.
    fn on_done(&self) -> Option<Option<Instant>> {
        let mut g = self.lock();
        let t = g.committed.pop_front()?;
        Some((!t.final_recorded).then_some(t.stamps.stt_commit).flatten())
    }
}

fn since_ms(later: Instant, earlier: Option<Instant>) -> f64 {
    earlier.map_or(0.0, |e| {
        later.saturating_duration_since(e).as_secs_f64() * 1000.0
    })
}

async fn relay(
    mut rx: mpsc::Receiver<SttEvent>,
    tracker: Arc<TurnTracker>,
    out: mpsc::Sender<SttSegment>,
    room: RoomId,
    speaker: ParticipantId,
) {
    let mut seq = 0u64;
    let mut out_open = true;
    while let Some(ev) = rx.recv().await {
        let now = Instant::now();
        match ev {
            SttEvent::Segment(mut seg) => {
                if seg.text.trim().is_empty() {
                    continue;
                }
                let info = tracker.on_segment(!seg.is_early, now);
                if info.uncommitted && ends_sentence(&seg.text) {
                    tracker.set_sentence_hint();
                }
                if info.record_first {
                    record_stage_latency(
                        STAGE_STT_FIRST_TEXT,
                        since_ms(now, info.stamps.stt_commit),
                    );
                }
                if info.record_final {
                    record_stage_latency(STAGE_STT_FINAL, since_ms(now, info.stamps.stt_commit));
                }
                seg.room = room.clone();
                seg.speaker = speaker.clone();
                seg.seq = seq;
                seg.utterance_seq = info.utterance_seq;
                seg.stamps = info.stamps;
                seq += 1;
                // Awaiting here is the backpressure path: STT -> sink fills up, never audio.
                if out_open && out.send(seg).await.is_err() {
                    out_open = false;
                    debug!(%room, %speaker, "segment receiver gone; discarding output");
                }
            }
            SttEvent::UtteranceDone { ok } => {
                if let Some(Some(commit)) = tracker.on_done() {
                    record_stage_latency(STAGE_STT_FINAL, since_ms(now, Some(commit)));
                }
                record_stage_outcome(STAGE_STT, if ok { Outcome::Ok } else { Outcome::Error });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------------------------

enum Slot {
    Empty,
    Opening(JoinHandle<Result<Box<dyn SttSession>, SttError>>),
    Ready(Box<dyn SttSession>),
}

struct Worker {
    spec: SpeakerSpec,
    stt: Arc<dyn SpeechToText>,
    vad: Box<dyn Vad + Send>,
    endpointer: Endpointer,
    /// Config snapshot of the current utterance; swapped only while idle.
    cfg: Arc<PipelineConfig>,
    cfg_rx: watch::Receiver<Arc<PipelineConfig>>,
    sink_tx: mpsc::Sender<SttEvent>,
    relay: JoinHandle<()>,
    tracker: Arc<TurnTracker>,
    slot: Slot,
    ring: VecDeque<(AudioFrame, f32)>,
    /// Arrival of the frame that opened the turn in progress.
    turn_start: Option<Instant>,
    last_speech_arrival: Instant,
    last_arrival: Instant,
    /// Samples of the turn in progress already fed live.
    streamed: usize,
    stream_broken: bool,
}

impl Worker {
    fn open(&self) -> JoinHandle<Result<Box<dyn SttSession>, SttError>> {
        let stt = self.stt.clone();
        let sink = self.sink_tx.clone();
        let params = SessionParams {
            room: self.spec.room.clone(),
            speaker: self.spec.id.clone(),
            lang_hint: self.spec.lang.clone(),
        };
        tokio::spawn(async move { stt.open_session(params, sink).await })
    }

    /// True when a session is ready. Awaits an in-progress (eager) open.
    async fn ensure_session(&mut self) -> bool {
        let handle = match mem::replace(&mut self.slot, Slot::Empty) {
            Slot::Ready(s) => {
                self.slot = Slot::Ready(s);
                return true;
            }
            Slot::Opening(h) => h,
            Slot::Empty => self.open(),
        };
        match handle.await {
            Ok(Ok(s)) => {
                self.slot = Slot::Ready(s);
                true
            }
            Ok(Err(e)) => {
                warn!(room = %self.spec.room, speaker = %self.spec.id, error = %e, "stt open failed");
                record_stage_outcome(STAGE_STT, Outcome::Error);
                false
            }
            Err(e) => {
                warn!(error = %e, "stt open task failed");
                false
            }
        }
    }

    fn apply_config_if_idle(&mut self) -> Vec<(EndpointEvent, Instant)> {
        if self.endpointer.is_speaking() || !self.cfg_rx.has_changed().unwrap_or(false) {
            return Vec::new();
        }
        let new = self.cfg_rx.borrow_and_update().clone();
        if new.endpointer != self.cfg.endpointer {
            // The endpointer has no setter; rebuild and replay the idle frames so the
            // pre-speech padding survives. The per-speaker energy baseline restarts (documented).
            self.endpointer = Endpointer::new(new.endpointer.clone());
            let mut events = Vec::new();
            for (f, p) in &self.ring {
                for e in self.endpointer.push(&f.frame.samples, *p) {
                    events.push((e, f.arrived_at));
                }
            }
            self.cfg = new;
            return events;
        }
        self.cfg = new;
        Vec::new()
    }

    async fn on_frame(&mut self, af: AudioFrame) {
        self.last_arrival = af.arrived_at;
        let replay = self.apply_config_if_idle();
        for (e, at) in replay {
            self.handle_event(e, at).await;
        }
        let prob = self.vad.speech_prob(&af.frame.samples);
        let events = self.endpointer.push(&af.frame.samples, prob);
        let at = af.arrived_at;
        if self.endpointer.is_speaking() || !events.is_empty() {
            self.ring.clear();
        } else {
            self.ring.push_back((af, prob));
            while self.ring.len() > REPLAY_FRAMES {
                self.ring.pop_front();
            }
        }
        let spoke = events
            .iter()
            .any(|e| matches!(e, EndpointEvent::LiveFrame { .. }));
        for e in events {
            self.handle_event(e, at).await;
        }
        self.apply_sentence_hint(spoke, at).await;
    }

    /// Early endpoint: a sentence terminator reached us from STT for the turn in progress.
    /// Ignored (and dropped) when the frame carried speech, else forwarded to the endpointer,
    /// which shortens the hangover (or closes the turn) when the feature is enabled.
    async fn apply_sentence_hint(&mut self, spoke: bool, at: Instant) {
        if !self.tracker.sentence_hint_pending() {
            return;
        }
        if spoke {
            self.tracker.clear_sentence_hint();
            return;
        }
        if !self.endpointer.is_speaking() {
            return;
        }
        self.tracker.clear_sentence_hint();
        for e in self.endpointer.hint_sentence_complete() {
            self.handle_event(e, at).await;
        }
    }

    async fn handle_event(&mut self, ev: EndpointEvent, at: Instant) {
        match ev {
            EndpointEvent::SpeechStart { .. } => {
                self.begin_turn(at);
                self.stream_broken = false;
                if self.cfg.streams_live() {
                    // Claim/await the session now so the first LiveFrame (which carries the
                    // pre-speech padding) is not delayed behind a connect.
                    self.ensure_session().await;
                }
            }
            EndpointEvent::LiveFrame { samples } => {
                if self.turn_start.is_none() {
                    // Continuation chunk after a cap split.
                    self.begin_turn(at);
                }
                self.last_speech_arrival = at;
                if self.cfg.streams_live() && !self.stream_broken {
                    self.feed(&samples).await;
                }
            }
            EndpointEvent::UtteranceEnd {
                audio,
                speech_ms,
                total_ms,
                reason,
                decided_at_ms,
                last_speech_end_ms,
                ..
            } => {
                self.commit(
                    audio,
                    speech_ms,
                    total_ms,
                    reason,
                    decided_at_ms,
                    last_speech_end_ms,
                )
                .await;
            }
            EndpointEvent::Dropped { reason, .. } => self.on_dropped(reason).await,
        }
    }

    fn begin_turn(&mut self, at: Instant) {
        self.turn_start = Some(at);
        self.streamed = 0;
        self.tracker.start_turn(at);
    }

    async fn feed(&mut self, samples: &[f32]) {
        if !self.ensure_session().await {
            self.stream_broken = true;
            self.streamed = 0;
            return;
        }
        let Slot::Ready(session) = &mut self.slot else {
            return;
        };
        match session.feed(samples).await {
            Ok(()) => self.streamed += samples.len(),
            Err(e) => {
                warn!(room = %self.spec.room, speaker = %self.spec.id, error = %e,
                    "stt feed failed; falling back to sending the closed utterance at commit");
                self.stream_broken = true;
                self.streamed = 0;
                self.slot = Slot::Empty; // reopen at commit
            }
        }
    }

    async fn commit(
        &mut self,
        audio: Vec<f32>,
        speech_ms: u32,
        total_ms: u32,
        reason: EndReason,
        decided_at_ms: u64,
        last_speech_end_ms: u64,
    ) {
        let decided = Instant::now();
        let speech_end = self.last_speech_arrival;
        let arrived = self.turn_start.unwrap_or(speech_end);
        if reason == EndReason::Silence {
            record_stage_latency(STAGE_ENDPOINT_DECISION, since_ms(decided, Some(speech_end)));
        }
        let streamed = if self.stream_broken { 0 } else { self.streamed };
        self.turn_start = None;
        self.streamed = 0;
        self.stream_broken = false;

        if !self.ensure_session().await {
            self.tracker.discard_current();
            warn!(room = %self.spec.room, speaker = %self.spec.id, "utterance lost: no stt session");
            return;
        }
        // Stamp after the session is ready: stt_* stages measure STT, not connect time.
        let commit_at = Instant::now();
        let seq = self.tracker.seq();
        let mut stamps = Stamps::new(arrived);
        stamps.speech_end = Some(speech_end);
        stamps.stt_commit = Some(commit_at);
        let req = CommitRequest {
            utterance: Utterance {
                room: self.spec.room.clone(),
                speaker: self.spec.id.clone(),
                seq,
                lang: self.spec.lang.clone(),
                audio: audio.into(),
                speech_ms,
                total_ms,
                hangover_ms: decided_at_ms.saturating_sub(last_speech_end_ms) as u32,
                reason,
                stamps,
            },
            mode: self.cfg.stt_mode,
            streamed_samples: streamed,
        };
        self.tracker.commit(speech_end, commit_at);
        let Slot::Ready(session) = &mut self.slot else {
            return;
        };
        if let Err(e) = session.commit(req).await {
            warn!(room = %self.spec.room, speaker = %self.spec.id, error = %e, "stt commit failed");
            record_stage_outcome(STAGE_STT, Outcome::Error);
            self.tracker.abort_last_commit();
            self.tracker.discard_current();
            self.slot = Slot::Empty;
        }
    }

    async fn on_dropped(&mut self, reason: DropReason) {
        debug!(room = %self.spec.room, speaker = %self.spec.id, ?reason, "utterance gated out");
        if self.streamed > 0
            && let Slot::Ready(session) = &mut self.slot
            && session.discard().await.is_err()
        {
            self.slot = Slot::Empty;
        }
        self.tracker.discard_current();
        self.turn_start = None;
        self.streamed = 0;
        self.stream_broken = false;
    }

    /// Input closed and drained: flush the open turn, close STT gracefully, wait for the relay.
    async fn finish(mut self) {
        let grace = Duration::from_millis(self.cfg.shutdown_grace_ms);
        let at = self.last_arrival;
        for e in self.endpointer.finish() {
            self.handle_event(e, at).await;
        }
        let session = match mem::replace(&mut self.slot, Slot::Empty) {
            Slot::Ready(s) => Some(s),
            Slot::Opening(h) => tokio::time::timeout(grace, h)
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(Result::ok),
            Slot::Empty => None,
        };
        if let Some(s) = session
            && tokio::time::timeout(grace, s.close()).await.is_err()
        {
            warn!(room = %self.spec.room, speaker = %self.spec.id, "stt close timed out");
        }
        drop(self.sink_tx);
        let mut relay = self.relay;
        if tokio::time::timeout(grace, &mut relay).await.is_err() {
            relay.abort();
        }
    }
}
