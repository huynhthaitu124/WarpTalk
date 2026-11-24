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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use warptalk_audio::resample;
use warptalk_ingress::{EndReason, EndpointEvent, Endpointer, EnergyVad, FRAME_SAMPLES, Vad};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, Stamps, SttEvent, SttMode, SttSegment, SttSession,
    Utterance,
};

use crate::config::{ChannelConfig, EchoAction, EchoGuardConfig, EngineConfig};
use crate::echo::{self, LevelHistory};
use crate::queue::{Chunk, ChunkQueue};
use crate::sink::SegmentSink;
use crate::types::{Channel, CopilotEvent, SegmentRecord, StatusKind};

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
}

pub(crate) struct WorkerCtx {
    pub channel: Channel,
    pub session_id: String,
    pub cfg_rx: watch::Receiver<ChannelConfig>,
    pub engine: Arc<EngineConfig>,
    pub echo: Arc<EchoGuardConfig>,
    pub queue: Arc<ChunkQueue>,
    pub stt: Arc<dyn SpeechToText>,
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
        }
    }

    pub(crate) async fn run(mut self) {
        let queue = self.ctx.queue.clone();
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
                self.live = None;
                self.stream_ok = false;
                self.next_open_after = Some(Instant::now());
                self.error("stt_feed", e.to_string());
                self.status(StatusKind::SttUnavailable, Some(e.to_string()));
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
        let utt = Utterance {
            room: self.ctx.session_id.clone(),
            speaker: self.ctx.channel.as_str().to_string(),
            seq: meta.seq,
            lang: self.cfg.language_hint.clone(),
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
        match l.session.commit(req).await {
            Ok(()) => self.pending.push_back(meta),
            Err(e) => {
                self.live = None;
                self.next_open_after = Some(Instant::now());
                self.error("stt_commit", e.to_string());
                self.status(StatusKind::SttUnavailable, Some(e.to_string()));
            }
        }
        self.rotate_if_old().await;
    }

    // ---- STT session ---------------------------------------------------------------------

    async fn ensure_session(&mut self) -> bool {
        if self.live.is_some() {
            return true;
        }
        if self.next_open_after.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        self.gen_counter += 1;
        let gen_id = self.gen_counter;
        let (tx, mut rx) = mpsc::channel::<SttEvent>(64);
        let params = SessionParams {
            room: self.ctx.session_id.clone(),
            speaker: self.ctx.channel.as_str().to_string(),
            lang_hint: self.cfg.language_hint.clone(),
        };
        let open_timeout = self.ctx.open_timeout;
        let res = tokio::time::timeout(open_timeout, self.ctx.stt.open_session(params, tx)).await;
        match res {
            Ok(Ok(session)) => {
                let fwd = self.fwd_tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = rx.recv().await {
                        if fwd.send((gen_id, ev)).await.is_err() {
                            break;
                        }
                    }
                });
                self.ctx
                    .counters
                    .sessions_opened
                    .fetch_add(1, Ordering::Relaxed);
                self.live = Some(Live {
                    session,
                    opened_at: Instant::now(),
                    gen_id,
                });
                self.next_open_after = None;
                self.status(StatusKind::SttConnected, None);
                true
            }
            other => {
                let msg = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "open timed out".to_string(),
                };
                self.next_open_after = Some(
                    Instant::now() + Duration::from_millis(self.ctx.engine.reconnect_backoff_ms),
                );
                self.error("stt_open", msg.clone());
                self.status(StatusKind::SttUnavailable, Some(msg));
                false
            }
        }
    }

    /// At an utterance boundary: replace an old session by a fresh one (old one drains in the
    /// background; its remaining events are matched by generation).
    async fn rotate_if_old(&mut self) {
        let max = Duration::from_millis(self.ctx.engine.session_rotate_after_ms);
        let old = self
            .live
            .as_ref()
            .is_some_and(|l| l.opened_at.elapsed() >= max);
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
                }
                self.emit(CopilotEvent::Partial {
                    channel: self.ctx.channel,
                    text: String::new(),
                });
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
        let (start, end, echo_flag, seq, partial) = match meta {
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
                (start, end, m.likely_echo, m.seq, m.text.clone())
            }
            None => (now_tl, now_tl, false, self.seq, text.clone()),
        };
        let off = self.cfg.start_offset_ms;
        let rec = SegmentRecord {
            client_segment_id: SegmentRecord::new_id(),
            channel: self.ctx.channel,
            text,
            language: seg.lang.clone(),
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
        if seg.is_early {
            self.emit(CopilotEvent::Partial {
                channel: self.ctx.channel,
                text: partial,
            });
        }
        let ends_sentence = rec
            .text
            .trim_end()
            .ends_with(['.', '!', '?', '。', '！', '？']);
        if !(echo_flag && self.ctx.echo.action == EchoAction::Drop) {
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

fn frames_ms(samples_16k: usize) -> u64 {
    (samples_16k as u64) * 1000 / u64::from(warptalk_audio::SAMPLE_RATE_16K)
}
