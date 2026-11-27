//! One audio channel: chunks -> 16 kHz -> endpointer -> streaming STT session -> segments.
//!
//! A single tokio task per channel does everything sequentially, so per-channel order is
//! structural. The audio path never waits on it (see `ChunkQueue`).
//!
//! STT session lifecycle: opened eagerly at start (off the critical path), kept for the whole
//! meeting, and *rotated* at an utterance boundary once older than `session_rotate_after_ms`
//! (the replacement is connected immediately, with a fresh/cached-until-near-expiry ephemeral
//! token). A failed open is retried after `reconnect_backoff_ms`; an utterance whose live
//! streaming was impossible is sent whole at commit (closed-chunk fallback). Events from every
//! session are tagged with a generation so a draining old session cannot be confused with its
//! successor.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::debug;
use warptalk_audio::resample;
use warptalk_ingress::{EndReason, EndpointEvent, Endpointer, EnergyVad, FRAME_SAMPLES, Vad};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, Stamps, SttEvent, SttMode, SttSegment, SttSession,
    Utterance,
};

use crate::config::{
    ChannelConfig, EchoAction, EchoGuardConfig, EngineConfig, FallbackConfig, LanguageConfig,
    SttRouting,
};
use crate::echo::{self, LevelHistory};
use crate::queue::{Chunk, ChunkQueue};
use crate::sink::SegmentSink;
use crate::stt::TimingBoard;
use crate::types::{Channel, CopilotEvent, SegmentRecord, StatusKind, SttRoute};

/// Non-blocking event emitter (drops, and counts, when the consumer is too slow).
#[derive(Clone)]
pub(crate) struct EventTx {
    tx: mpsc::Sender<CopilotEvent>,
    dropped: Arc<AtomicU64>,
}

impl EventTx {
    pub(crate) fn new(tx: mpsc::Sender<CopilotEvent>, dropped: Arc<AtomicU64>) -> Self {
        Self { tx, dropped }
    }

    pub(crate) fn emit(&self, ev: CopilotEvent) {
        if self.tx.try_send(ev).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
pub(crate) struct Counters {
    pub utterances: AtomicU64,
    pub segments: AtomicU64,
    pub echo_marked: AtomicU64,
    pub stt_errors: AtomicU64,
    pub sessions_opened: AtomicU64,
    pub fallbacks: AtomicU64,
    /// 0 = direct, 1 = server.
    pub route: AtomicU8,
}

impl Counters {
    pub(crate) fn route(&self) -> SttRoute {
        if self.route.load(Ordering::Relaxed) == 1 {
            SttRoute::Server
        } else {
            SttRoute::Direct
        }
    }
}

/// Where a channel's STT session runs. `Server` sessions produce segments the server has
/// already ingested: they are shown but never posted to the sink (no double posting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Direct,
    Server,
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Route::Direct => "direct",
            Route::Server => "server",
        }
    }
}

pub(crate) struct WorkerCtx {
    pub channel: Channel,
    pub session_id: String,
    pub cfg_rx: watch::Receiver<ChannelConfig>,
    pub engine: Arc<EngineConfig>,
    pub echo: Arc<EchoGuardConfig>,
    pub queue: Arc<ChunkQueue>,
    /// Direct provider path (unused when `routing` is `Server`).
    pub stt: Arc<dyn SpeechToText>,
    /// Server socket path (`Auto` fallback and `Server`).
    pub server_stt: Option<Arc<dyn SpeechToText>>,
    pub routing: SttRouting,
    pub fallback: Arc<FallbackConfig>,
    pub language: Arc<LanguageConfig>,
    pub timing: Arc<TimingBoard>,
    pub sink: Arc<dyn SegmentSink>,
    pub events: EventTx,
    pub my_hist: Arc<Mutex<LevelHistory>>,
    pub other_hist: Arc<Mutex<LevelHistory>>,
    pub counters: Arc<Counters>,
    pub open_timeout: Duration,
}

struct Live {
    session: Box<dyn SttSession>,
    opened_at: Instant,
    gen_id: u64,
    route: Route,
}

struct Meta {
    seq: u64,
    gen_id: u64,
    /// Push-timeline ms (without `start_offset_ms`).
    start_ms: u64,
    end_ms: Option<u64>,
    last_seg_end: u64,
    likely_echo: bool,
    text: String,
}

pub(crate) struct Worker {
    ctx: WorkerCtx,
    cfg: ChannelConfig,
    ep: Endpointer,
    vad: EnergyVad,
    frame_buf: Vec<f32>,
    /// Push-timeline minus endpointer time (grows when audio was dropped upstream).
    gap_ms: u64,
    live: Option<Live>,
    gen_counter: u64,
    next_open_after: Option<Instant>,
    fwd_tx: mpsc::Sender<(u64, SttEvent)>,
    fwd_rx: mpsc::Receiver<(u64, SttEvent)>,
    open: Option<Meta>,
    pending: VecDeque<Meta>,
    seq: u64,
    streamed: usize,
    stream_ok: bool,
    lvl_sumsq: f64,
    lvl_n: u64,
    lvl_last_ms: u64,
    route: Route,
    /// Recent direct failures (for the fallback threshold).
    direct_failures: VecDeque<Instant>,
    /// Live (uncommitted) text of the utterance being spoken now; re-shown after a committed
    /// utterance's segments replace the overlay's partial line.
    live_partial: String,
    retry_direct_at: Option<Instant>,
    gen_routes: HashMap<u64, Route>,
}

impl Worker {
    pub(crate) fn new(ctx: WorkerCtx) -> Self {
        let cfg = ctx.cfg_rx.borrow().clone();
        let (fwd_tx, fwd_rx) = mpsc::channel(256);
        Self {
            ep: Endpointer::new(cfg.endpointer.clone()),
            vad: EnergyVad {
                full_scale_rms: cfg.vad_full_scale_rms,
            },
            cfg,
            ctx,
            frame_buf: Vec::with_capacity(FRAME_SAMPLES * 2),
            gap_ms: 0,
            live: None,
            gen_counter: 0,
            next_open_after: None,
            fwd_tx,
            fwd_rx,
            open: None,
            pending: VecDeque::new(),
            seq: 0,
            streamed: 0,
            stream_ok: true,
            lvl_sumsq: 0.0,
            lvl_n: 0,
            lvl_last_ms: 0,
            route: Route::Direct,
            direct_failures: VecDeque::new(),
            live_partial: String::new(),
            retry_direct_at: None,
            gen_routes: HashMap::new(),
        }
    }

    pub(crate) async fn run(mut self) {
        let queue = self.ctx.queue.clone();
        if self.ctx.routing == SttRouting::Server && self.ctx.server_stt.is_some() {
            self.route = Route::Server;
            self.ctx.counters.route.store(1, Ordering::Relaxed);
        }
        self.ensure_session().await;
        loop {
            tokio::select! {
                biased;
                Some((g, ev)) = self.fwd_rx.recv() => self.on_stt(g, ev).await,
                chunk = queue.pop() => match chunk {
                    Some(c) => self.on_chunk(c).await,
                    None => break,
                },
            }
        }
        // End of stream: flush a trailing partial frame and any open turn.
        if !self.frame_buf.is_empty() {
            let mut last = std::mem::take(&mut self.frame_buf);
            last.resize(FRAME_SAMPLES, 0.0);
            self.push_frame(&last).await;
        }
        let evs = self.ep.finish();
        self.process(evs).await;
        // Drain in-flight transcripts.
        let deadline = Instant::now() + Duration::from_millis(self.ctx.engine.drain_timeout_ms);
        while !self.pending.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.fwd_rx.recv()).await {
                Ok(Some((g, ev))) => self.on_stt(g, ev).await,
                _ => break,
            }
        }
        if let Some(l) = self.live.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), l.session.close()).await;
        }
    }

    fn emit(&self, ev: CopilotEvent) {
        self.ctx.events.emit(ev);
    }

    fn status(&self, kind: StatusKind, detail: Option<String>) {
        self.emit(CopilotEvent::Status {
            channel: Some(self.ctx.channel),
            kind,
            detail,
        });
    }

    fn error(&self, code: &str, message: String) {
        self.ctx.counters.stt_errors.fetch_add(1, Ordering::Relaxed);
        self.emit(CopilotEvent::Error {
            channel: Some(self.ctx.channel),
            code: code.into(),
            message,
        });
    }

    fn timeline_ms(&self) -> u64 {
        self.ep.now_ms() + self.gap_ms + frames_ms(self.frame_buf.len())
    }

    // ---- audio ------------------------------------------------------------------------

    async fn on_chunk(&mut self, c: Chunk) {
        // Hot reload, only while idle (the endpointer holds per-turn state).
        if !self.ep.is_speaking() && self.ctx.cfg_rx.has_changed().unwrap_or(false) {
            let new = self.ctx.cfg_rx.borrow_and_update().clone();
            self.ep = Endpointer::new(new.endpointer.clone());
            self.vad = EnergyVad {
                full_scale_rms: new.vad_full_scale_rms,
            };
            self.cfg = new;
            self.frame_buf.clear();
            self.gap_ms = c.start_ms; // fresh endpointer clock starts at this chunk
        }
        let gap = c.start_ms as i64 - self.timeline_ms() as i64;
        if gap > self.ctx.engine.gap_tolerance_ms as i64 {
            self.gap_ms += gap as u64;
        }
        let pcm = resample(&c.samples, c.sample_rate, warptalk_audio::SAMPLE_RATE_16K);
        self.level(&pcm);
        self.frame_buf.extend_from_slice(&pcm);
        let mut off = 0;
        while self.frame_buf.len() - off >= FRAME_SAMPLES {
            let frame: Vec<f32> = self.frame_buf[off..off + FRAME_SAMPLES].to_vec();
            off += FRAME_SAMPLES;
            self.push_frame(&frame).await;
        }
        self.frame_buf.drain(..off);
    }

    fn level(&mut self, pcm: &[f32]) {
        let interval = self.ctx.engine.levels_interval_ms;
        if interval == 0 {
            return;
        }
        self.lvl_sumsq += warptalk_audio::sum_squares(pcm);
        self.lvl_n += pcm.len() as u64;
        let now = self.timeline_ms();
        if now.saturating_sub(self.lvl_last_ms) >= interval && self.lvl_n > 0 {
            let rms = (self.lvl_sumsq / self.lvl_n as f64).sqrt() as f32;
            self.emit(CopilotEvent::Levels {
                channel: self.ctx.channel,
                rms,
            });
            self.lvl_sumsq = 0.0;
            self.lvl_n = 0;
            self.lvl_last_ms = now;
        }
    }

    async fn push_frame(&mut self, frame: &[f32]) {
        let prob = self.vad.speech_prob(frame);
        let evs = self.ep.push(frame, prob);
        if !evs.is_empty() {
            self.process(evs).await;
        }
    }

    // ---- endpoint events -----------------------------------------------------------------

    async fn process(&mut self, events: Vec<EndpointEvent>) {
        for ev in events {
            match ev {
                EndpointEvent::SpeechStart { t_ms } => {
                    self.begin_utterance(t_ms + self.gap_ms);
                    self.status(StatusKind::SpeechStart, None);
                    self.ensure_session().await;
                }
                EndpointEvent::LiveFrame { samples } => {
                    if self.open.is_none() {
                        // Continuation after a chunk cap: a new turn without SpeechStart.
                        let start = (self.ep.now_ms() + self.gap_ms)
                            .saturating_sub(frames_ms(samples.len()));
                        self.begin_utterance(start);
                    }
                    self.feed(&samples).await;
                }
                EndpointEvent::UtteranceEnd {
                    audio,
                    speech_ms,
                    total_ms,
                    reason,
                    last_speech_end_ms,
                    decided_at_ms,
                    ..
                } => {
                    let end = last_speech_end_ms + self.gap_ms;
                    self.end_utterance(audio, speech_ms, total_ms, reason, end, decided_at_ms)
                        .await;
                }
                EndpointEvent::Dropped { .. } => {
                    self.open = None;
                    self.streamed = 0;
                    self.stream_ok = true;
                    if let Some(l) = self.live.as_mut() {
                        let _ = l.session.discard().await;
                    }
                }
            }
        }
    }

    fn begin_utterance(&mut self, start_ms: u64) {
        self.open = Some(Meta {
            seq: self.seq,
            gen_id: 0,
            start_ms,
            end_ms: None,
            last_seg_end: start_ms,
            likely_echo: false,
            text: String::new(),
        });
        self.seq += 1;
        self.streamed = 0;
        self.stream_ok = true;
    }

    async fn feed(&mut self, samples: &[f32]) {
        let Some(l) = self.live.as_mut() else {
            self.stream_ok = false;
            return;
        };
        match l.session.feed(samples).await {
            Ok(()) => self.streamed += samples.len(),
            Err(e) => {
                let route = l.route;
                self.live = None;
                self.stream_ok = false;
                self.next_open_after = Some(Instant::now());
                self.error("stt_feed", e.to_string());
                self.status(StatusKind::SttUnavailable, Some(e.to_string()));
                if route == Route::Direct {
                    self.note_direct_failure(&e.to_string());
                }
            }
        }
    }

    async fn end_utterance(
        &mut self,
        audio: Vec<f32>,
        speech_ms: u32,
        total_ms: u32,
        reason: EndReason,
        end_ms: u64,
        decided_at_ms: u64,
    ) {
        let mut meta = match self.open.take() {
            Some(m) => m,
            None => {
                let start = end_ms.saturating_sub(u64::from(speech_ms));
                self.seq += 1;
                Meta {
                    seq: self.seq - 1,
                    gen_id: 0,
                    start_ms: start,
                    end_ms: None,
                    last_seg_end: start,
                    likely_echo: false,
                    text: String::new(),
                }
            }
        };
        meta.end_ms = Some(end_ms.max(meta.start_ms));
        self.ctx.counters.utterances.fetch_add(1, Ordering::Relaxed);
        self.status(StatusKind::SpeechEnd, None);

        if self.ctx.channel == Channel::Me && self.ctx.echo.enabled {
            let me = self
                .ctx
                .my_hist
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let others = self
                .ctx
                .other_hist
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            meta.likely_echo = echo::evaluate(&me, &others, meta.start_ms, end_ms, &self.ctx.echo);
        }

        // A session that died or never opened gets one more chance now; the whole utterance
        // is then sent at commit.
        let streamed_ok = self.stream_ok && self.live.is_some();
        if self.live.is_none() {
            self.ensure_session().await;
        }
        let streamed = if streamed_ok { self.streamed } else { 0 };
        self.streamed = 0;
        self.stream_ok = true;

        let Some(l) = self.live.as_mut() else {
            self.error("stt_unavailable", "no STT session; utterance lost".into());
            return;
        };
        meta.gen_id = l.gen_id;
        let now = Instant::now();
        if l.route == Route::Server {
            self.ctx.timing.set(
                self.ctx.channel,
                meta.seq,
                meta.start_ms + self.cfg.start_offset_ms,
                end_ms.max(meta.start_ms) + self.cfg.start_offset_ms,
            );
        }
        let utt = Utterance {
            room: self.ctx.session_id.clone(),
            speaker: self.ctx.channel.as_str().to_string(),
            seq: meta.seq,
            lang: self.ctx.language.hint(self.cfg.language_hint.as_deref()),
            audio: audio.into(),
            speech_ms,
            total_ms,
            hangover_ms: self.ep.now_ms().saturating_sub(decided_at_ms) as u32,
            reason,
            stamps: Stamps::new(now),
        };
        let req = CommitRequest {
            utterance: utt,
            mode: SttMode::Realtime,
            streamed_samples: streamed,
        };
        let route = l.route;
        // Whatever is live now belongs to this utterance; the next one starts empty.
        self.live_partial.clear();
        match l.session.commit(req).await {
            Ok(()) => self.pending.push_back(meta),
            Err(e) => {
                self.live = None;
                self.next_open_after = Some(Instant::now());
                self.error("stt_commit", e.to_string());
                self.status(StatusKind::SttUnavailable, Some(e.to_string()));
                if route == Route::Direct {
                    self.note_direct_failure(&e.to_string());
                }
            }
        }
        self.rotate_if_old().await;
    }

    // ---- STT session ---------------------------------------------------------------------

    fn backend(&self, route: Route) -> Option<Arc<dyn SpeechToText>> {
        match route {
            Route::Direct => (self.ctx.routing != SttRouting::Server).then(|| self.ctx.stt.clone()),
            Route::Server => self.ctx.server_stt.clone(),
        }
    }

    /// Open a session on `route`'s backend (not installed yet).
    async fn open_live(&mut self, route: Route, timeout: Duration) -> Result<Live, String> {
        let stt = self
            .backend(route)
            .ok_or_else(|| format!("no {} STT backend", route.name()))?;
        self.gen_counter += 1;
        let gen_id = self.gen_counter;
        let (tx, mut rx) = mpsc::channel::<SttEvent>(64);
        let params = SessionParams {
            room: self.ctx.session_id.clone(),
            speaker: self.ctx.channel.as_str().to_string(),
            lang_hint: self.ctx.language.hint(self.cfg.language_hint.as_deref()),
        };
        match tokio::time::timeout(timeout, stt.open_session(params, tx)).await {
            Ok(Ok(session)) => {
                let fwd = self.fwd_tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = rx.recv().await {
                        if fwd.send((gen_id, ev)).await.is_err() {
                            break;
                        }
                    }
                });
                self.gen_routes.insert(gen_id, route);
                self.ctx
                    .counters
                    .sessions_opened
                    .fetch_add(1, Ordering::Relaxed);
                Ok(Live {
                    session,
                    opened_at: Instant::now(),
                    gen_id,
                    route,
                })
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("open timed out".to_string()),
        }
    }

    fn set_route(&mut self, route: Route) {
        self.route = route;
        self.ctx
            .counters
            .route
            .store(u8::from(route == Route::Server), Ordering::Relaxed);
    }

    async fn ensure_session(&mut self) -> bool {
        if self.live.is_some() {
            return true;
        }
        if self.next_open_after.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        let route = self.route;
        match self.open_live(route, self.ctx.open_timeout).await {
            Ok(live) => {
                self.live = Some(live);
                self.next_open_after = None;
                self.status(StatusKind::SttConnected, Some(route.name().to_string()));
                true
            }
            Err(msg) => {
                self.next_open_after = Some(
                    Instant::now() + Duration::from_millis(self.ctx.engine.reconnect_backoff_ms),
                );
                let code = if msg.ends_with("http 401") {
                    "auth_required"
                } else {
                    "stt_open"
                };
                self.error(code, msg.clone());
                self.status(StatusKind::SttUnavailable, Some(msg.clone()));
                match route {
                    Route::Direct => {
                        // Token issue / connect failures count towards the fallback; on the
                        // threshold the server session is opened right away.
                        if self.note_direct_failure(&msg) {
                            self.next_open_after = None;
                            return Box::pin(self.ensure_session()).await;
                        }
                    }
                    Route::Server => {
                        // In `auto` the server is no better: go back to trying direct.
                        if self.ctx.routing == SttRouting::Auto {
                            self.set_route(Route::Direct);
                            self.retry_direct_at = None;
                            self.direct_failures.clear();
                        }
                    }
                }
                false
            }
        }
    }

    /// Records a failure of the direct path; in `auto` mode, once the threshold is reached
    /// within the window, switches this channel to the server. Returns whether it switched.
    fn note_direct_failure(&mut self, reason: &str) -> bool {
        if self.ctx.routing != SttRouting::Auto
            || self.ctx.server_stt.is_none()
            || self.route != Route::Direct
        {
            return false;
        }
        let now = Instant::now();
        let window = Duration::from_millis(self.ctx.fallback.window_ms);
        self.direct_failures.push_back(now);
        while self
            .direct_failures
            .front()
            .is_some_and(|t| now.duration_since(*t) > window)
        {
            self.direct_failures.pop_front();
        }
        if (self.direct_failures.len() as u32) < self.ctx.fallback.failure_threshold {
            return false;
        }
        self.direct_failures.clear();
        self.set_route(Route::Server);
        self.retry_direct_at =
            Some(now + Duration::from_millis(self.ctx.fallback.retry_direct_after_ms));
        self.ctx.counters.fallbacks.fetch_add(1, Ordering::Relaxed);
        if let Some(l) = self.live.take() {
            // In-flight transcripts of the old session still drain (matched by generation).
            tokio::spawn(async move { l.session.close().await });
        }
        self.status(
            StatusKind::SttFallback,
            Some(format!("direct->server: {reason}")),
        );
        true
    }

    /// While on the server in `auto` mode: after `retry_direct_after_ms`, probe the direct
    /// path at an utterance boundary and switch back if it opens.
    async fn maybe_recover_direct(&mut self) {
        if self.route != Route::Server
            || self.ctx.routing != SttRouting::Auto
            || self.ep.is_speaking()
            || !self.retry_direct_at.is_some_and(|t| Instant::now() >= t)
        {
            return;
        }
        let probe = Duration::from_millis(self.ctx.fallback.probe_timeout_ms);
        match self.open_live(Route::Direct, probe).await {
            Ok(live) => {
                if let Some(old) = self.live.take() {
                    tokio::spawn(async move { old.session.close().await });
                }
                self.live = Some(live);
                self.set_route(Route::Direct);
                self.retry_direct_at = None;
                self.direct_failures.clear();
                self.next_open_after = None;
                self.status(StatusKind::SttConnected, Some("direct".into()));
                self.status(StatusKind::SttRecovered, Some("server->direct".into()));
            }
            Err(msg) => {
                debug!(error = %msg, "direct STT still unavailable; staying on the server");
                self.retry_direct_at = Some(
                    Instant::now() + Duration::from_millis(self.ctx.fallback.retry_direct_after_ms),
                );
            }
        }
    }

    /// At an utterance boundary: replace an old direct session by a fresh one (old one drains
    /// in the background; its remaining events are matched by generation). Server sessions
    /// reconnect their shared socket by themselves and are never rotated. Also the moment a
    /// server-routed channel probes the direct path again.
    async fn rotate_if_old(&mut self) {
        self.maybe_recover_direct().await;
        let max = Duration::from_millis(self.ctx.engine.session_rotate_after_ms);
        let old = self
            .live
            .as_ref()
            .is_some_and(|l| l.route == Route::Direct && l.opened_at.elapsed() >= max);
        if old && !self.ep.is_speaking() {
            if let Some(l) = self.live.take() {
                tokio::spawn(async move { l.session.close().await });
            }
            self.ensure_session().await;
        }
    }

    // ---- STT output ----------------------------------------------------------------------

    async fn on_stt(&mut self, gen_id: u64, ev: SttEvent) {
        match ev {
            SttEvent::Segment(seg) => self.on_segment(gen_id, seg).await,
            SttEvent::UtteranceDone { ok } => {
                if let Some(i) = self.pending.iter().position(|m| m.gen_id == gen_id) {
                    self.pending.remove(i);
                }
                if !ok {
                    self.error("stt_failed", "transcription failed for an utterance".into());
                    if self.gen_routes.get(&gen_id) == Some(&Route::Direct) {
                        self.note_direct_failure("transcription failed");
                    }
                } else if self.gen_routes.get(&gen_id) == Some(&Route::Direct) {
                    self.direct_failures.clear();
                }
                self.emit(CopilotEvent::Partial {
                    channel: self.ctx.channel,
                    text: self.live_partial.clone(),
                });
            }
            SttEvent::Partial { text } => {
                let ends_sentence = ends_sentence(&text);
                self.live_partial = text;
                self.emit(CopilotEvent::Partial {
                    channel: self.ctx.channel,
                    text: self.live_partial.clone(),
                });
                // The speaker finished a sentence: end the turn on a short pause instead of
                // the full hangover (no-op unless `vad_sentence_hint_hangover_ms` > 0).
                if ends_sentence {
                    let evs = self.ep.hint_sentence_complete();
                    if !evs.is_empty() {
                        Box::pin(self.process(evs)).await;
                    }
                }
            }
        }
    }

    async fn on_segment(&mut self, gen_id: u64, seg: SttSegment) {
        let text = seg.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let now_tl = self.timeline_ms();
        let est = self.ctx.engine.est_ms_per_char;
        let committed = self.pending.iter().position(|m| m.gen_id == gen_id);
        let meta = match committed {
            Some(i) => self.pending.get_mut(i),
            None => self.open.as_mut(),
        };
        let (start, end, echo_flag, seq) = match meta {
            Some(m) => {
                let start = m.last_seg_end.max(m.start_ms);
                let bound = m.end_ms.unwrap_or(now_tl).max(start);
                let end = match (seg.is_early, m.end_ms) {
                    (false, Some(e)) => e.max(start),
                    _ => (start + text.chars().count() as u64 * est).min(bound),
                };
                m.last_seg_end = end;
                if !m.text.is_empty() {
                    m.text.push(' ');
                }
                m.text.push_str(&text);
                (start, end, m.likely_echo, m.seq)
            }
            None => (now_tl, now_tl, false, self.seq),
        };
        let off = self.cfg.start_offset_ms;
        let language = self.ctx.language.resolve(&text, &seg.lang);
        let via_server = self.gen_routes.get(&gen_id) == Some(&Route::Server);
        let rec = SegmentRecord {
            client_segment_id: SegmentRecord::new_id(),
            channel: self.ctx.channel,
            text,
            language,
            start_ms: start + off,
            end_ms: end + off,
            confidence: seg.confidence,
            is_final: true,
            likely_echo: echo_flag,
            utterance_seq: seq,
        };
        self.ctx.counters.segments.fetch_add(1, Ordering::Relaxed);
        if echo_flag {
            self.ctx
                .counters
                .echo_marked
                .fetch_add(1, Ordering::Relaxed);
        }
        self.emit(CopilotEvent::Segment {
            segment: rec.clone(),
        });
        // The segment replaced the overlay's partial line: put the live text of the utterance
        // being spoken now back (empty clears it).
        self.emit(CopilotEvent::Partial {
            channel: self.ctx.channel,
            text: self.live_partial.clone(),
        });
        let ends_sentence = ends_sentence(&rec.text);
        // Segments from the server path were already ingested by the server (same dedup
        // store, usage and credit gate as `POST /segments`): posting them again would
        // double them in the transcript, so they are only shown.
        if !via_server && !(echo_flag && self.ctx.echo.action == EchoAction::Drop) {
            self.ctx.sink.submit(rec).await;
        }
        // Semantic endpointing hint (no-op unless `vad_sentence_hint_hangover_ms` > 0).
        if seg.is_early && ends_sentence {
            let evs = self.ep.hint_sentence_complete();
            if !evs.is_empty() {
                Box::pin(self.process(evs)).await;
            }
        }
    }
}

/// `sang .` (the stream stopped inside `.NET`) is not a finished sentence.
fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end();
    if let Some(before) = t.strip_suffix('.') {
        return before.chars().last().is_some_and(|c| !c.is_whitespace());
    }
    t.ends_with(['!', '?', '。', '！', '？'])
}

fn frames_ms(samples_16k: usize) -> u64 {
    (samples_16k as u64) * 1000 / u64::from(warptalk_audio::SAMPLE_RATE_16K)
}
