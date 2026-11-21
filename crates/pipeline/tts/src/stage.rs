//! [`TtsStage`]: Translation in, streamed [`DubChunk`]s out.
//!
//! # Latency design decisions
//!
//! - **Streamed chunk by chunk**: every PCM chunk the vendor emits is forwarded the moment it
//!   arrives; a sentence is never buffered whole (the cache copy is a side tee). The sentence
//!   ends with an `is_last` chunk that may be empty, so no chunk is ever held back "to know it
//!   is the last one".
//! - **Warm sockets** come from the vendor client's pool; this stage opens a context and the
//!   slot is taken AFTER the open so a cold dial is not spent holding a slot.
//! - **Slot gate** (`max_concurrency`, default 3) covers a context's whole life (first push to
//!   confirmed end), never playout; the wait is `tts_slot_wait` (every sentence, zeros included).
//! - **Continuity without waste**: a context stays open for the next sentence only when that
//!   sentence is already queued (prosody carries on); otherwise it is closed at once, in the
//!   background, because an idle open context holds a vendor slot.
//! - **Cache first** (`tts:cache:{sha256}`): a hit skips the vendor (about 2 ms against about 1 s).
//!   The key includes voice, delivery and sample rate.
//! - **Synthesize once** per (room, speaker, language) sentence and fan out downstream (ADR D5);
//!   a duplicate request for the same (segment, sentence, language) from another listener is
//!   dropped.
//! - **Order**: one worker per (room, speaker, language) speaks strictly in arrival order; a
//!   retry happens inside the worker so it can make a line late but never out of order.
//! - **Isochrony + catch-up**: speed is centred on the speaker's measured dub/source ratio and
//!   boosted when the dub is behind (1.0 at 1500 ms lag up to 1.25 at 4000 ms); nothing is
//!   dropped, a deep backlog is caught up with speed, not shed.
//! - **Degradation ladder** clone -> standard -> text only, each step a `RouteDegraded` event;
//!   retries are bounded and only for transient errors; a partially spoken line is never
//!   replayed; give-ups are dead-lettered.
//!
//! Metrics (`warptalk_stage_latency_ms`): `tts_first_audio` (first attempt start to first audio
//! forwarded), `tts_synthesis` (worker busy time), `tts_slot_wait`; outcomes under stage `tts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, warn};
use uuid::Uuid;
use warptalk_mt::Translation;
use warptalk_telemetry::{Outcome, record_stage_latency, record_stage_outcome};

use crate::cache::cache_key;
use crate::config::TtsConfig;
use crate::delivery::{DubFit, gen_config, observe};
use crate::error::TtsError;
use crate::gate::{SlotGate, SlotPermit};
use crate::ports::{CachedAudio, ContextParams, GenConfig, SynthContext, Synthesizer, TtsCache};
use crate::types::{DubChunk, RouteTier, TtsEvent, TtsStamps, VoiceInfo, VoiceMode};
use crate::voice::VoiceResolver;

pub const STAGE_TTS_FIRST_AUDIO: &str = "tts_first_audio";
pub const STAGE_TTS_SYNTHESIS: &str = "tts_synthesis";
pub const STAGE_TTS_SLOT_WAIT: &str = "tts_slot_wait";
const STAGE: &str = "tts";

pub struct TtsDeps {
    pub synth: Arc<dyn Synthesizer>,
    pub cache: Arc<dyn TtsCache>,
    pub voices: Arc<VoiceResolver>,
}

pub struct TtsOutputs {
    /// Bounded: backpressure towards the vendor stream.
    pub chunks: mpsc::Sender<DubChunk>,
    /// Best effort (`try_send`).
    pub events: Option<mpsc::Sender<TtsEvent>>,
}

type Key = (String, String, String);
type DedupeKey = (String, String, String, Uuid, u32);

struct Shared {
    deps: TtsDeps,
    cfg: watch::Receiver<TtsConfig>,
    out: TtsOutputs,
    gate: Arc<SlotGate>,
    seen: Mutex<HashMap<DedupeKey, Instant>>,
}

pub struct TtsStage;

pub struct TtsStageHandle {
    join: JoinHandle<()>,
    gate: Arc<SlotGate>,
}

impl TtsStageHandle {
    /// Resolves when the input closed and every queued sentence was spoken.
    pub async fn join(self) {
        let _ = self.join.await;
    }
    pub fn slots_in_use(&self) -> usize {
        self.gate.in_use()
    }
}

impl TtsStage {
    pub fn spawn(
        deps: TtsDeps,
        cfg: watch::Receiver<TtsConfig>,
        input: mpsc::Receiver<Translation>,
        out: TtsOutputs,
    ) -> TtsStageHandle {
        let gate = SlotGate::new(cfg.borrow().max_concurrency);
        let shared = Arc::new(Shared {
            deps,
            cfg,
            out,
            gate: gate.clone(),
            seen: Mutex::new(HashMap::new()),
        });
        let join = tokio::spawn(dispatch(shared, input));
        TtsStageHandle { join, gate }
    }
}

async fn dispatch(sh: Arc<Shared>, mut input: mpsc::Receiver<Translation>) {
    let mut cfg_rx = sh.cfg.clone();
    let mut workers: HashMap<Key, mpsc::Sender<Translation>> = HashMap::new();
    let mut tasks: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            t = input.recv() => {
                let Some(t) = t else { break };
                route(&sh, &mut workers, &mut tasks, t).await;
            }
            Ok(()) = cfg_rx.changed() => {
                let n = cfg_rx.borrow_and_update().max_concurrency;
                sh.gate.set_limit(n);
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    drop(workers);
    while tasks.join_next().await.is_some() {}
}

fn is_duplicate(sh: &Shared, t: &Translation, ttl: Duration) -> bool {
    let mut seen = sh.seen.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    if seen.len() > 4096 {
        seen.retain(|_, at| now.duration_since(*at) < ttl);
    }
    let k = (
        t.room.clone(),
        t.speaker.clone(),
        t.target_lang.clone(),
        t.segment_id,
        t.chunk_idx,
    );
    match seen.get(&k) {
        Some(at) if now.duration_since(*at) < ttl => true,
        _ => {
            seen.insert(k, now);
            false
        }
    }
}

async fn route(
    sh: &Arc<Shared>,
    workers: &mut HashMap<Key, mpsc::Sender<Translation>>,
    tasks: &mut JoinSet<()>,
    t: Translation,
) {
    let cfg = sh.cfg.borrow().clone();
    // Same-language lines are captions only; empty text has nothing to say.
    if t.passthrough || t.text.trim().is_empty() {
        return;
    }
    if is_duplicate(sh, &t, Duration::from_millis(cfg.dedupe_ttl_ms)) {
        debug!(segment = %t.segment_id, "duplicate synthesis request dropped (fan-out)");
        metrics_dedup();
        return;
    }
    let key: Key = (t.room.clone(), t.speaker.clone(), t.target_lang.clone());
    let mut msg = t;
    loop {
        if let Some(tx) = workers.get(&key) {
            match tx.send(msg).await {
                Ok(()) => return,
                Err(e) => msg = e.0, // the worker went idle between lookup and send
            }
        }
        let (tx, rx) = mpsc::channel(cfg.key_inbox_capacity.max(1));
        workers.insert(key.clone(), tx);
        let w = Worker {
            sh: sh.clone(),
            rx,
            open: None,
            last_tier: None,
            fit: DubFit::NONE,
        };
        tasks.spawn(w.run());
    }
}

fn metrics_dedup() {
    record_stage_outcome("tts_fanout_dedup", Outcome::Ok);
}

struct OpenCtx {
    ctx: Box<dyn SynthContext>,
    voice_id: String,
    lang: String,
    permit: SlotPermit,
}

/// Mutable state of one sentence across attempts.
#[derive(Default)]
struct SentenceState {
    seq: u32,
    first_audio: Option<Instant>,
    pcm: Vec<f32>,
    slot_wait: Duration,
    continued: bool,
    samples: usize,
}

struct Worker {
    sh: Arc<Shared>,
    rx: mpsc::Receiver<Translation>,
    open: Option<OpenCtx>,
    last_tier: Option<RouteTier>,
    fit: DubFit,
}

/// Why a failed sentence is NOT retried, or `None` to try again. In this order: a half-spoken
/// line or a request the vendor refused is never retried however much budget is left.
pub fn give_up_reason(
    err: &TtsError,
    partial: bool,
    failures: u32,
    first_attempt: Instant,
    cfg: &TtsConfig,
) -> Option<String> {
    if partial {
        return Some("partially_spoken".into());
    }
    if !err.is_transient() {
        return Some("not_retryable".into());
    }
    if failures > cfg.retry.max_retries {
        return Some("retries_exhausted".into());
    }
    if first_attempt.elapsed() >= Duration::from_millis(cfg.retry.window_ms) {
        return Some("retry_window_exceeded".into());
    }
    None
}

impl Worker {
    async fn run(mut self) {
        loop {
            let idle = Duration::from_millis(self.sh.cfg.borrow().key_idle_ms);
            match tokio::time::timeout(idle, self.rx.recv()).await {
                Ok(Some(t)) => self.process(t).await,
                Ok(None) => break,
                Err(_) => {
                    // Idle exit: close the inbox, then speak whatever raced in.
                    self.rx.close();
                    while let Some(t) = self.rx.recv().await {
                        self.process(t).await;
                    }
                    break;
                }
            }
        }
        self.end_context();
    }

    fn event(&self, ev: TtsEvent) {
        if let Some(tx) = &self.sh.out.events
            && tx.try_send(ev).is_err()
        {
            warn!("tts event channel full or closed; event dropped");
        }
    }

    fn degrade(&mut self, t: &Translation, from: RouteTier, to: RouteTier, reason: String) {
        if self.last_tier == Some(to) {
            return; // already reported for this route
        }
        self.last_tier = Some(to);
        self.event(TtsEvent::RouteDegraded {
            room: t.room.clone(),
            speaker: t.speaker.clone(),
            target_lang: t.target_lang.clone(),
            from,
            to,
            reason,
        });
    }

    /// Ends the open context in the background; its slot is released when the vendor confirms.
    fn end_context(&mut self) {
        if let Some(o) = self.open.take() {
            tokio::spawn(async move {
                o.ctx.close().await;
                drop(o.permit);
            });
        }
    }

    fn abort_context(&mut self) {
        if let Some(o) = self.open.take() {
            tokio::spawn(async move {
                o.ctx.cancel().await;
                drop(o.permit);
            });
        }
    }

    fn chunk(
        &self,
        t: &Translation,
        voice: &VoiceInfo,
        pcm: Vec<f32>,
        st: &SentenceState,
        t_start: Instant,
        is_last: bool,
        cache_hit: bool,
    ) -> DubChunk {
        DubChunk {
            segment_id: t.segment_id,
            chunk_idx: t.chunk_idx,
            room: t.room.clone(),
            speaker: t.speaker.clone(),
            target_lang: t.target_lang.clone(),
            voice: voice.clone(),
            pcm: pcm.into(),
            sample_rate: self.sh.deps.synth.sample_rate(),
            seq: st.seq,
            is_last,
            cache_hit,
            stamps: TtsStamps {
                mt: t.stamps,
                tts_start: t_start,
                tts_first_audio: st.first_audio,
            },
        }
    }

    async fn process(&mut self, t: Translation) {
        let cfg = self.sh.cfg.borrow().clone();
        let t_start = Instant::now();
        // Lag: how far behind the speaker this dub starts (0 when the turn end is unknown, e.g.
        // an early sentence of a turn still in progress).
        let lag_ms = t.stamps.stt.speech_end.map_or(0, |e| {
            t_start.saturating_duration_since(e).as_millis() as u64
        });

        let resolved = self
            .sh
            .deps
            .voices
            .resolve(&t.room, &t.speaker, &t.target_lang, &t.text)
            .await;
        let mut voice = resolved.voice;
        if let Some(reason) = resolved.clone_degraded {
            self.degrade(&t, RouteTier::Clone, RouteTier::Standard, reason);
        }
        let gen_cfg = gen_config(self.fit, lag_ms, &cfg);
        let rate = self.sh.deps.synth.sample_rate();

        // Cache first.
        if cfg.cache.enabled {
            let key = cache_key(
                &t.speaker,
                &t.target_lang,
                &t.text,
                voice.mode.as_str(),
                &voice.voice_id,
                rate,
                &gen_cfg,
            );
            if let Some(hit) = self.sh.deps.cache.get(&key).await {
                if hit.sample_rate == rate && self.replay(&t, &voice, &hit, &cfg, t_start).await {
                    record_stage_latency(STAGE_TTS_SLOT_WAIT, 0.0);
                    record_stage_latency(
                        STAGE_TTS_FIRST_AUDIO,
                        t_start.elapsed().as_secs_f64() * 1000.0,
                    );
                    record_stage_outcome(STAGE, Outcome::Ok);
                    self.finish_ok(&t, &voice, hit.pcm.len(), true, Duration::ZERO, false);
                }
                return;
            }
        }

        let mut st = SentenceState::default();
        let first_attempt = Instant::now();
        let mut failures = 0u32;
        let mut degraded_once = false;
        let outcome: Result<(), (TtsError, String)> = loop {
            match self
                .attempt(&t, &voice, &gen_cfg, &cfg, &mut st, t_start)
                .await
            {
                Ok(()) => break Ok(()),
                Err(TtsError::Cancelled) => return, // consumer gone: nothing to degrade
                Err(e) => {
                    let partial = st.seq > 0;
                    if voice.mode == VoiceMode::Cloned && !partial && !degraded_once {
                        // Clone failure: speak this line in the stock voice, no retry spent.
                        degraded_once = true;
                        self.degrade(
                            &t,
                            RouteTier::Clone,
                            RouteTier::Standard,
                            format!("clone_synthesis_failed: {e}"),
                        );
                        voice = self
                            .sh
                            .deps
                            .voices
                            .standard(&t.room, &t.speaker, &t.target_lang)
                            .await;
                        continue;
                    }
                    failures += 1;
                    match give_up_reason(&e, partial, failures, first_attempt, &cfg) {
                        None => {
                            warn!(error = %e, attempt = failures, "synthesis failed, retrying");
                            let backoff = cfg.retry_backoff() * 2u32.pow(failures - 1);
                            tokio::time::sleep(backoff).await;
                        }
                        Some(reason) => break Err((e, reason)),
                    }
                }
            }
        };

        record_stage_latency(STAGE_TTS_SLOT_WAIT, st.slot_wait.as_secs_f64() * 1000.0);
        record_stage_latency(
            STAGE_TTS_SYNTHESIS,
            t_start.elapsed().as_secs_f64() * 1000.0,
        );
        match outcome {
            Ok(()) => {
                if let Some(fa) = st.first_audio {
                    record_stage_latency(
                        STAGE_TTS_FIRST_AUDIO,
                        fa.saturating_duration_since(t_start).as_secs_f64() * 1000.0,
                    );
                }
                record_stage_outcome(STAGE, Outcome::Ok);
                // Terminal marker: the audio was already forwarded as it arrived.
                let last = self.chunk(&t, &voice, Vec::new(), &st, t_start, true, false);
                if self.sh.out.chunks.send(last).await.is_err() {
                    return;
                }
                if cfg.cache.enabled && st.samples > 0 {
                    let key = cache_key(
                        &t.speaker,
                        &t.target_lang,
                        &t.text,
                        voice.mode.as_str(),
                        &voice.voice_id,
                        rate,
                        &gen_cfg,
                    );
                    let audio = CachedAudio {
                        sample_rate: rate,
                        pcm: std::mem::take(&mut st.pcm).into(),
                    };
                    let cache = self.sh.deps.cache.clone();
                    let ttl = cfg.cache_ttl();
                    tokio::spawn(async move { cache.put(&key, &audio, ttl).await });
                }
                let dub_ms = (st.samples as u64 * 1000 / u64::from(rate.max(1))) as u32;
                if let Some(src) = t.source_ms {
                    self.fit = observe(self.fit, src, dub_ms, &cfg.isochrony);
                }
                self.finish_ok(&t, &voice, st.samples, false, st.slot_wait, st.continued);
            }
            Err((e, reason)) => {
                record_stage_outcome(STAGE, e.outcome());
                record_stage_outcome(STAGE, Outcome::DeadLetter);
                if st.seq > 0 {
                    // Close the half-spoken stream so the egress is not left waiting.
                    let last = self.chunk(&t, &voice, Vec::new(), &st, t_start, true, false);
                    let _ = self.sh.out.chunks.send(last).await;
                }
                let from = if voice.mode == VoiceMode::Cloned {
                    RouteTier::Clone
                } else {
                    RouteTier::Standard
                };
                self.degrade(&t, from, RouteTier::TextOnly, format!("{reason}: {e}"));
                self.event(TtsEvent::DeadLetter {
                    segment_id: t.segment_id,
                    chunk_idx: t.chunk_idx,
                    room: t.room.clone(),
                    speaker: t.speaker.clone(),
                    target_lang: t.target_lang.clone(),
                    attempts: failures,
                    reason,
                    error: e.to_string(),
                });
            }
        }
    }

    fn finish_ok(
        &mut self,
        t: &Translation,
        voice: &VoiceInfo,
        samples: usize,
        cache_hit: bool,
        slot_wait: Duration,
        continued: bool,
    ) {
        self.last_tier = Some(if voice.mode == VoiceMode::Cloned {
            RouteTier::Clone
        } else {
            RouteTier::Standard
        });
        let rate = u64::from(self.sh.deps.synth.sample_rate().max(1));
        self.event(TtsEvent::SentenceDone {
            segment_id: t.segment_id,
            chunk_idx: t.chunk_idx,
            room: t.room.clone(),
            speaker: t.speaker.clone(),
            target_lang: t.target_lang.clone(),
            voice: voice.clone(),
            duration_ms: (samples as u64 * 1000 / rate) as u32,
            cache_hit,
            slot_wait_ms: slot_wait.as_millis() as u64,
            continued,
        });
    }

    /// Replays a cached sentence in stream-shaped chunks. `false` when the consumer is gone.
    async fn replay(
        &mut self,
        t: &Translation,
        voice: &VoiceInfo,
        hit: &CachedAudio,
        cfg: &TtsConfig,
        t_start: Instant,
    ) -> bool {
        let per = (hit.sample_rate as usize * cfg.cache.replay_chunk_ms as usize / 1000).max(1);
        let mut st = SentenceState::default();
        let n = hit.pcm.len().div_ceil(per);
        for (i, part) in hit.pcm.chunks(per).enumerate() {
            st.first_audio.get_or_insert_with(Instant::now);
            let c = self.chunk(t, voice, part.to_vec(), &st, t_start, i + 1 == n, true);
            if self.sh.out.chunks.send(c).await.is_err() {
                return false;
            }
            st.seq += 1;
        }
        true
    }

    /// One synthesis attempt on a (reused or fresh) context, streaming into the output.
    async fn attempt(
        &mut self,
        t: &Translation,
        voice: &VoiceInfo,
        gen_cfg: &GenConfig,
        cfg: &TtsConfig,
        st: &mut SentenceState,
        t_start: Instant,
    ) -> Result<(), TtsError> {
        let idle = cfg.context_idle_expiry();
        let reusable = self.open.as_ref().is_some_and(|o| {
            o.voice_id == voice.voice_id && o.lang == t.target_lang && o.ctx.is_reusable(idle)
        });
        if reusable {
            st.continued = true;
        } else {
            self.end_context();
            let ctx = self
                .sh
                .deps
                .synth
                .open_context(&ContextParams {
                    language: t.target_lang.clone(),
                    voice_id: voice.voice_id.clone(),
                })
                .await?;
            // Slot AFTER the open: a cold dial is not spent holding a slot.
            let permit = self.sh.gate.acquire().await;
            st.slot_wait += permit.waited;
            self.open = Some(OpenCtx {
                ctx,
                voice_id: voice.voice_id.clone(),
                lang: t.target_lang.clone(),
                permit,
            });
        }
        let Some(mut open) = self.open.take() else {
            return Err(TtsError::Transient("context vanished".into()));
        };

        let (ptx, mut prx) = mpsc::channel::<Vec<f32>>(32);
        let keep_pcm = cfg.cache.enabled;
        let speak = async {
            let r = open.ctx.speak(&t.text, gen_cfg, &ptx).await;
            drop(ptx);
            r
        };
        let sh = self.sh.clone();
        let forward = async {
            while let Some(pcm) = prx.recv().await {
                st.first_audio.get_or_insert_with(Instant::now);
                st.samples += pcm.len();
                if keep_pcm {
                    st.pcm.extend_from_slice(&pcm);
                }
                let chunk = DubChunk {
                    segment_id: t.segment_id,
                    chunk_idx: t.chunk_idx,
                    room: t.room.clone(),
                    speaker: t.speaker.clone(),
                    target_lang: t.target_lang.clone(),
                    voice: voice.clone(),
                    pcm: pcm.into(),
                    sample_rate: sh.deps.synth.sample_rate(),
                    seq: st.seq,
                    is_last: false,
                    cache_hit: false,
                    stamps: TtsStamps {
                        mt: t.stamps,
                        tts_start: t_start,
                        tts_first_audio: st.first_audio,
                    },
                };
                if sh.out.chunks.send(chunk).await.is_err() {
                    return false; // consumer gone: dropping prx cancels the speak
                }
                st.seq += 1;
            }
            true
        };
        let (res, consumer_ok) = tokio::join!(speak, forward);
        if !consumer_ok {
            open.ctx.cancel().await;
            drop(open.permit);
            return Err(TtsError::Cancelled);
        }
        match res {
            Ok(_) => {
                let keep =
                    cfg.context.continuity && !self.rx.is_empty() && open.ctx.is_reusable(idle);
                if keep {
                    self.open = Some(open);
                } else {
                    tokio::spawn(async move {
                        open.ctx.close().await;
                        drop(open.permit);
                    });
                }
                Ok(())
            }
            Err(e) => {
                tokio::spawn(async move {
                    open.ctx.cancel().await;
                    drop(open.permit);
                });
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn give_up_order_and_bounds() {
        let cfg = TtsConfig::default();
        let now = Instant::now();
        let tr = TtsError::Transient("drop".into());
        assert_eq!(
            give_up_reason(&tr, true, 1, now, &cfg).as_deref(),
            Some("partially_spoken")
        );
        assert_eq!(
            give_up_reason(
                &TtsError::Permanent("bad voice".into()),
                false,
                1,
                now,
                &cfg
            )
            .as_deref(),
            Some("not_retryable")
        );
        assert_eq!(give_up_reason(&tr, false, 1, now, &cfg), None);
        assert_eq!(
            give_up_reason(&tr, false, 2, now, &cfg).as_deref(),
            Some("retries_exhausted")
        );
        let old = now.checked_sub(Duration::from_secs(11)).unwrap_or(now);
        assert_eq!(
            give_up_reason(&tr, false, 1, old, &cfg).as_deref(),
            Some("retry_window_exceeded")
        );
    }
}
