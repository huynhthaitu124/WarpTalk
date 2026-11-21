//! [`TranslationStage`]: the task between STT and TTS.
//!
//! Input: [`SttSegment`]s (per speaker in order). Output: one [`Translation`] per sentence per
//! target language of the room.
//!
//! # Latency design decisions
//!
//! - **Fan-out per target language is concurrent**; within a segment the first sentence is
//!   translated alone (Realtime, warm pool) while sentences 1..N go in ONE batched call that is
//!   already running by the time the first lands. The first sentence is published the moment it
//!   is ready, so its dub starts while the rest is still translating.
//! - **Publish order, not compute order**: a per-(room, speaker, language) [`OrderGate`] makes a
//!   later segment wait (bounded by `publish_order_wait`) for the earlier one's publish only;
//!   translation itself never waits. Nothing is dropped when the bound elapses.
//! - **Speculative prefetch** from early hints is claimed at validation time; a hit skips the
//!   model call, a still-running call is awaited rather than duplicated.
//! - **Evidence-gated prompt**: glossary terms only when they occur in the source; the
//!   ASR-repair clause only with a suspected mishearing, meeting context or low confidence.
//! - **Skip before spend**: control markers, empty text, paused / not started / no credits and
//!   same-language checks run before any model call.
//! - **Backpressure**: at most `max_in_flight` segments are processed concurrently; the input
//!   channel then fills, which pushes back on STT (never on audio).
//! - Ports (`TargetLanguages`, `SkipPolicy`, `ContextProvider`, `GlossaryProvider`) are awaited
//!   in segment order and must be cached.
//!
//! Metrics (`warptalk_stage_latency_ms`): `mt_first_token` (stage receipt to first model text,
//! first sentence only), `mt_done` (stage receipt to each sentence ready), `mt_order_wait`
//! (time a finished first sentence waited behind its predecessor); outcomes under stage `mt`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use warptalk_contracts::events::is_control_marker;
use warptalk_pipeline_runtime::SttSegment;
use warptalk_telemetry::{Outcome, record_stage_latency, record_stage_outcome};

use crate::config::MtConfig;
use crate::error::MtError;
use crate::glossary::{GlossaryProvider, GlossaryTerm, has_suspected_mishearing, select_terms};
use crate::lang::{base_language, is_same_language};
use crate::order::{OrderGate, Ticket};
use crate::polish::choose_transcript;
use crate::ports::{ContextProvider, SkipPolicy, TargetLanguages};
use crate::prompt::{BatchRequest, TranslateRequest};
use crate::speculative::SpeculativeCache;
use crate::text::{normalize_key, split_sentences};
use crate::translator::{TranslateOutput, Translator};
use crate::types::{MtEvent, MtStamps, SkipReason, SpeculativeHint, Translation};

pub const STAGE_MT_FIRST_TOKEN: &str = "mt_first_token";
pub const STAGE_MT_DONE: &str = "mt_done";
pub const STAGE_MT_ORDER_WAIT: &str = "mt_order_wait";
const STAGE: &str = "mt";

pub struct MtDeps {
    pub translator: Arc<dyn Translator>,
    pub glossary: Arc<dyn GlossaryProvider>,
    pub targets: Arc<dyn TargetLanguages>,
    pub skip: Arc<dyn SkipPolicy>,
    pub context: Arc<dyn ContextProvider>,
}

pub struct MtOutputs {
    /// Bounded: backpressure towards the translation tasks (and so towards STT).
    pub translations: mpsc::Sender<Translation>,
    /// Best effort (`try_send`): an event can never delay a sentence.
    pub events: Option<mpsc::Sender<MtEvent>>,
}

#[derive(Default)]
struct RoomState {
    recent: VecDeque<String>,
    static_ctx: Option<Arc<Vec<String>>>,
}

struct Inner {
    deps: MtDeps,
    cfg: watch::Receiver<MtConfig>,
    out: MtOutputs,
    spec: SpeculativeCache,
    order: OrderGate,
    rooms: Mutex<HashMap<String, RoomState>>,
    /// (room, speaker) -> (utterance_seq, an early segment of it was seen).
    utterances: Mutex<HashMap<(String, String), (u64, bool)>>,
}

struct SegmentJob {
    seg: SttSegment,
    sentences: Vec<String>,
    source_ms: Vec<Option<u32>>,
    context: Vec<String>,
    received_at: Instant,
}

pub struct TranslationStage;

pub struct StageHandle {
    join: JoinHandle<()>,
    inner: Arc<Inner>,
}

impl StageHandle {
    /// Resolves once the input channel closed and every in-flight segment finished.
    pub async fn join(self) {
        let _ = self.join.await;
    }

    /// Entries currently held by the speculative cache.
    pub fn speculative_entries(&self) -> usize {
        self.inner.spec.len()
    }

    /// Drops ordering state of a finished room.
    pub fn forget_room(&self, room: &str) {
        self.inner.order.forget_room(room);
        self.inner
            .rooms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(room);
        self.inner
            .utterances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(r, _), _| r != room);
    }
}

impl TranslationStage {
    /// Spawns the stage. `max_in_flight` and the speculative slot count are read once here; every
    /// other knob is re-read from `cfg` per segment (hot reload).
    pub fn spawn(
        deps: MtDeps,
        cfg: watch::Receiver<MtConfig>,
        input: mpsc::Receiver<SttSegment>,
        hints: Option<mpsc::Receiver<SpeculativeHint>>,
        out: MtOutputs,
    ) -> StageHandle {
        let initial = cfg.borrow().clone();
        let inner = Arc::new(Inner {
            deps,
            spec: SpeculativeCache::new(initial.speculative.max_concurrent),
            cfg,
            out,
            order: OrderGate::new(),
            rooms: Mutex::new(HashMap::new()),
            utterances: Mutex::new(HashMap::new()),
        });
        let join = tokio::spawn(run(inner.clone(), initial.max_in_flight, input, hints));
        StageHandle { join, inner }
    }
}

async fn run(
    inner: Arc<Inner>,
    max_in_flight: usize,
    mut input: mpsc::Receiver<SttSegment>,
    mut hints: Option<mpsc::Receiver<SpeculativeHint>>,
) {
    let sem = Arc::new(Semaphore::new(max_in_flight.max(1)));
    loop {
        let hint_fut = async {
            match hints.as_mut() {
                Some(h) => h.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            seg = input.recv() => {
                let Some(seg) = seg else { break };
                handle_segment(&inner, &sem, seg).await;
            }
            hint = hint_fut => match hint {
                Some(h) => handle_hint(&inner, h).await,
                None => hints = None,
            },
        }
    }
    // Drain: every in-flight segment holds a permit.
    let _ = sem.acquire_many(max_in_flight.max(1) as u32).await;
}

fn emit_event(inner: &Inner, ev: MtEvent) {
    if let Some(tx) = &inner.out.events {
        let _ = tx.try_send(ev);
    }
}

fn dedupe_targets(targets: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    targets
        .into_iter()
        .filter(|t| !t.trim().is_empty() && seen.insert(base_language(t)))
        .collect()
}

async fn context_for(inner: &Inner, room: &str, cfg: &MtConfig) -> Vec<String> {
    let cached = inner
        .rooms
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(room)
        .and_then(|r| r.static_ctx.clone());
    let static_ctx = match cached {
        Some(c) => c,
        None => {
            let fetched = Arc::new(inner.deps.context.meeting_context(room).await);
            inner
                .rooms
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(room.to_string())
                .or_default()
                .static_ctx = Some(fetched.clone());
            fetched
        }
    };
    let rooms = inner.rooms.lock().unwrap_or_else(PoisonError::into_inner);
    let recent = rooms
        .get(room)
        .map(|r| r.recent.clone())
        .unwrap_or_default();
    let take = cfg.recent_context_segments.min(recent.len());
    let mut ctx: Vec<String> = recent.iter().skip(recent.len() - take).cloned().collect();
    ctx.extend(static_ctx.iter().cloned());
    let drop_n = ctx.len().saturating_sub(cfg.context_lines);
    ctx.drain(..drop_n);
    ctx
}

fn remember(inner: &Inner, seg: &SttSegment, cfg: &MtConfig) {
    // Only trustworthy utterances become context for later ones.
    if seg
        .confidence
        .is_some_and(|c| c < cfg.context_min_confidence)
    {
        return;
    }
    let mut rooms = inner.rooms.lock().unwrap_or_else(PoisonError::into_inner);
    let r = rooms.entry(seg.room.clone()).or_default();
    r.recent.push_back(seg.text.trim().to_string());
    while r.recent.len() > cfg.recent_context_segments.max(1) {
        r.recent.pop_front();
    }
}

/// Source speech duration per sentence, apportioned by characters. Only for a segment that is
/// the whole turn (not early, and no early segment of the same utterance before it).
fn source_ms_per_sentence(
    inner: &Inner,
    seg: &SttSegment,
    sentences: &[String],
) -> Vec<Option<u32>> {
    let saw_early = {
        let mut map = inner
            .utterances
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let e = map
            .entry((seg.room.clone(), seg.speaker.clone()))
            .or_insert((seg.utterance_seq, false));
        if e.0 != seg.utterance_seq {
            *e = (seg.utterance_seq, false);
        }
        let before = e.1;
        if seg.is_early {
            e.1 = true;
        }
        before
    };
    let total_ms = match (seg.stamps.speech_end, seg.is_early || saw_early) {
        (Some(end), false) => end
            .checked_duration_since(seg.stamps.audio_arrived)
            .map(|d| d.as_millis() as u64),
        _ => None,
    };
    let chars: Vec<u64> = sentences.iter().map(|s| s.chars().count() as u64).collect();
    let sum: u64 = chars.iter().sum();
    chars
        .iter()
        .map(|c| match total_ms {
            Some(t) if sum > 0 => Some((t * c / sum) as u32),
            _ => None,
        })
        .collect()
}

async fn handle_segment(inner: &Arc<Inner>, sem: &Arc<Semaphore>, seg: SttSegment) {
    let received_at = Instant::now();
    let cfg = inner.cfg.borrow().clone();
    let skip = |reason| {
        emit_event(
            inner,
            MtEvent::Skipped {
                room: seg.room.clone(),
                speaker: seg.speaker.clone(),
                segment_id: seg.segment_id,
                reason,
            },
        );
    };
    if is_control_marker(seg.text.trim()) {
        return skip(SkipReason::ControlMarker);
    }
    let sentences = split_sentences(&seg.text);
    if sentences.is_empty() {
        return skip(SkipReason::Empty);
    }
    let targets = dedupe_targets(
        inner
            .deps
            .targets
            .targets(&seg.room, &seg.speaker, &seg.lang)
            .await,
    );
    if targets.is_empty() {
        return skip(SkipReason::NoTargets);
    }
    let context = context_for(inner, &seg.room, &cfg).await;
    remember(inner, &seg, &cfg);
    let source_ms = source_ms_per_sentence(inner, &seg, &sentences);

    let Ok(permit) = sem.clone().acquire_owned().await else {
        return;
    };
    // Tickets are taken here, in arrival order, before the segment is spawned.
    let tickets: Vec<(String, Ticket)> = targets
        .into_iter()
        .map(|lang| {
            let t =
                inner
                    .order
                    .ticket((seg.room.clone(), seg.speaker.clone(), base_language(&lang)));
            (lang, t)
        })
        .collect();
    let job = Arc::new(SegmentJob {
        seg,
        sentences,
        source_ms,
        context,
        received_at,
    });
    let inner = inner.clone();
    tokio::spawn(async move {
        let _permit = permit;
        run_segment(inner, job, tickets).await;
    });
}

async fn run_segment(inner: Arc<Inner>, job: Arc<SegmentJob>, tickets: Vec<(String, Ticket)>) {
    if let Some(reason) = inner
        .deps
        .skip
        .skip_reason(&job.seg.room, &job.seg.speaker)
        .await
    {
        debug!(?reason, room = %job.seg.room, "segment skipped before translation");
        emit_event(
            &inner,
            MtEvent::Skipped {
                room: job.seg.room.clone(),
                speaker: job.seg.speaker.clone(),
                segment_id: job.seg.segment_id,
                reason,
            },
        );
        return; // tickets drop: successors are released
    }
    futures::future::join_all(
        tickets
            .into_iter()
            .map(|(lang, ticket)| run_lang(inner.clone(), job.clone(), lang, ticket)),
    )
    .await;
}

struct SentenceResult {
    text: String,
    model: String,
    first_token_at: Instant,
    done_at: Instant,
}

impl From<TranslateOutput> for SentenceResult {
    fn from(o: TranslateOutput) -> Self {
        Self {
            text: o.text,
            model: o.model,
            first_token_at: o.first_token_at,
            done_at: o.done_at,
        }
    }
}

fn request_for(
    cfg: &MtConfig,
    job: &SegmentJob,
    lang: &str,
    terms: &[GlossaryTerm],
    text: String,
) -> TranslateRequest {
    let glossary = select_terms(&text, terms, cfg.glossary_limit);
    let repair = has_suspected_mishearing(&glossary)
        || !job.context.is_empty()
        || job
            .seg
            .confidence
            .is_some_and(|c| c < cfg.repair_confidence_floor);
    TranslateRequest {
        text,
        source_lang: job.seg.lang.clone(),
        target_lang: lang.to_string(),
        glossary,
        context: job.context.clone(),
        repair,
    }
}

fn spec_key(room: &str, speaker: &str, lang: &str, text: &str) -> (String, String, String, String) {
    (
        room.to_string(),
        speaker.to_string(),
        base_language(lang),
        normalize_key(text),
    )
}

async fn first_sentence(
    inner: Arc<Inner>,
    cfg: MtConfig,
    job: Arc<SegmentJob>,
    lang: String,
    terms: Arc<Vec<GlossaryTerm>>,
) -> Result<SentenceResult, MtError> {
    let text = job.sentences[0].clone();
    let ttl = Duration::from_millis(cfg.speculative.ttl_ms);
    if let Some(fut) = inner.spec.take(
        &spec_key(&job.seg.room, &job.seg.speaker, &lang, &text),
        ttl,
    ) {
        // A hit consumes the entry; a still-running call is awaited rather than duplicated.
        if let Some(out) = fut.await {
            debug!("speculative translation hit");
            return Ok(out.into());
        }
    }
    let req = request_for(&cfg, &job, &lang, &terms, text);
    inner.deps.translator.translate(&req).await.map(Into::into)
}

/// Sentences 1..N: speculative hits are used as they are, the misses go out in ONE batched call.
async fn rest_sentences(
    inner: Arc<Inner>,
    cfg: MtConfig,
    job: Arc<SegmentJob>,
    lang: String,
    terms: Arc<Vec<GlossaryTerm>>,
) -> Vec<Result<SentenceResult, MtError>> {
    let ttl = Duration::from_millis(cfg.speculative.ttl_ms);
    let rest = &job.sentences[1..];
    let mut results: Vec<Option<Result<SentenceResult, MtError>>> =
        (0..rest.len()).map(|_| None).collect();
    let mut misses: Vec<usize> = Vec::new();
    for (i, s) in rest.iter().enumerate() {
        let hit = inner
            .spec
            .take(&spec_key(&job.seg.room, &job.seg.speaker, &lang, s), ttl);
        match hit {
            Some(f) => match f.await {
                Some(o) => results[i] = Some(Ok(o.into())),
                None => misses.push(i),
            },
            None => misses.push(i),
        }
    }
    if !misses.is_empty() {
        let texts: Vec<String> = misses.iter().map(|&i| rest[i].clone()).collect();
        let first = request_for(&cfg, &job, &lang, &terms, texts.join("\n"));
        let req = BatchRequest {
            texts,
            source_lang: first.source_lang,
            target_lang: first.target_lang,
            glossary: first.glossary,
            context: first.context,
            repair: first.repair,
        };
        match inner.deps.translator.translate_batch(&req).await {
            Ok(b) => {
                for (k, &i) in misses.iter().enumerate() {
                    results[i] = Some(Ok(SentenceResult {
                        text: b.texts[k].clone(),
                        model: b.model.clone(),
                        first_token_at: b.done_at,
                        done_at: b.done_at,
                    }));
                }
            }
            Err(e) => {
                for &i in &misses {
                    results[i] = Some(Err(e.clone()));
                }
            }
        }
    }
    results
        .into_iter()
        .map(|r| r.unwrap_or(Err(MtError::Empty)))
        .collect()
}

async fn polish_one(
    inner: &Inner,
    cfg: &MtConfig,
    text: &str,
    lang: &str,
) -> (String, &'static str) {
    if !cfg.polish.enabled {
        return (text.to_string(), "passthrough");
    }
    let budget = Duration::from_millis(cfg.polish.timeout_ms);
    match tokio::time::timeout(budget, inner.deps.translator.polish(text, lang)).await {
        Ok(Ok(p)) => {
            let chosen = choose_transcript(text, &p);
            if chosen == text {
                (chosen, "passthrough")
            } else {
                (chosen, "passthrough+polish")
            }
        }
        // Raw text ships: a nicer-looking transcript must never delay or drop the line.
        _ => (text.to_string(), "passthrough"),
    }
}

async fn run_lang(inner: Arc<Inner>, job: Arc<SegmentJob>, lang: String, mut ticket: Ticket) {
    let cfg = inner.cfg.borrow().clone();
    let n = job.sentences.len();

    if is_same_language(&job.seg.lang, &lang) {
        let polished = futures::future::join_all(
            job.sentences
                .iter()
                .map(|s| polish_one(&inner, &cfg, s, &job.seg.lang)),
        )
        .await;
        wait_turn(&inner, &job, &lang, &mut ticket, &cfg).await;
        for (idx, (text, model)) in polished.into_iter().enumerate() {
            let now = Instant::now();
            let r = SentenceResult {
                text,
                model: model.to_string(),
                first_token_at: now,
                done_at: now,
            };
            if !publish(&inner, &job, &lang, idx, r, true).await {
                return;
            }
        }
        ticket.finish();
        return;
    }

    let terms = inner.deps.glossary.terms(&job.seg.room).await;
    let first_task = tokio::spawn(first_sentence(
        inner.clone(),
        cfg.clone(),
        job.clone(),
        lang.clone(),
        terms.clone(),
    ));
    let rest_task = (n > 1).then(|| {
        tokio::spawn(rest_sentences(
            inner.clone(),
            cfg.clone(),
            job.clone(),
            lang.clone(),
            terms.clone(),
        ))
    });

    let first = first_task
        .await
        .unwrap_or_else(|e| Err(MtError::Protocol(format!("task failed: {e}"))));
    wait_turn(&inner, &job, &lang, &mut ticket, &cfg).await;
    if !publish_result(&inner, &job, &lang, 0, first).await {
        return;
    }
    if let Some(rt) = rest_task {
        let rest = rt.await.unwrap_or_default();
        for (i, r) in rest.into_iter().enumerate() {
            if !publish_result(&inner, &job, &lang, i + 1, r).await {
                return;
            }
        }
    }
    ticket.finish();
}

async fn wait_turn(
    inner: &Inner,
    job: &SegmentJob,
    lang: &str,
    ticket: &mut Ticket,
    cfg: &MtConfig,
) {
    let t0 = Instant::now();
    if !ticket.wait_turn(cfg.publish_order_wait()).await {
        warn!(
            room = %job.seg.room,
            speaker = %job.seg.speaker,
            lang,
            "publish order wait exceeded; going out of turn (nothing dropped)"
        );
        emit_event(
            inner,
            MtEvent::OrderWaitExceeded {
                room: job.seg.room.clone(),
                speaker: job.seg.speaker.clone(),
                segment_id: job.seg.segment_id,
                target_lang: lang.to_string(),
            },
        );
    }
    record_stage_latency(STAGE_MT_ORDER_WAIT, t0.elapsed().as_secs_f64() * 1000.0);
}

async fn publish_result(
    inner: &Inner,
    job: &SegmentJob,
    lang: &str,
    idx: usize,
    res: Result<SentenceResult, MtError>,
) -> bool {
    match res {
        Ok(r) => publish(inner, job, lang, idx, r, false).await,
        Err(e) => {
            record_stage_outcome(STAGE, e.outcome());
            warn!(error = %e, room = %job.seg.room, lang, idx, "translation failed");
            emit_event(
                inner,
                MtEvent::Failed {
                    room: job.seg.room.clone(),
                    speaker: job.seg.speaker.clone(),
                    segment_id: job.seg.segment_id,
                    chunk_idx: idx as u32,
                    target_lang: lang.to_string(),
                    error: e.to_string(),
                },
            );
            true
        }
    }
}

/// Sends one translation. `false` when the consumer is gone (the stage then stops that task).
async fn publish(
    inner: &Inner,
    job: &SegmentJob,
    lang: &str,
    idx: usize,
    r: SentenceResult,
    passthrough: bool,
) -> bool {
    let stamps = MtStamps {
        stt: job.seg.stamps,
        mt_start: job.received_at,
        mt_first_token: Some(r.first_token_at),
        mt_done: Some(r.done_at),
    };
    if idx == 0
        && !passthrough
        && let Some(ms) = stamps.mt_first_token_ms()
    {
        record_stage_latency(STAGE_MT_FIRST_TOKEN, ms as f64);
    }
    if let Some(ms) = stamps.mt_done_ms() {
        record_stage_latency(STAGE_MT_DONE, ms as f64);
    }
    record_stage_outcome(STAGE, Outcome::Ok);
    let t = Translation {
        segment_id: job.seg.segment_id,
        chunk_idx: idx as u32,
        is_final_chunk: idx + 1 == job.sentences.len(),
        room: job.seg.room.clone(),
        speaker: job.seg.speaker.clone(),
        seq: job.seg.seq,
        utterance_seq: job.seg.utterance_seq,
        source_lang: job.seg.lang.clone(),
        target_lang: lang.to_string(),
        source_text: job.sentences[idx].clone(),
        text: r.text,
        model: r.model,
        passthrough,
        is_early: job.seg.is_early,
        confidence: job.seg.confidence,
        source_ms: job.source_ms.get(idx).copied().flatten(),
        stamps,
    };
    inner.out.translations.send(t).await.is_ok()
}

async fn handle_hint(inner: &Arc<Inner>, hint: SpeculativeHint) {
    let cfg = inner.cfg.borrow().clone();
    if !cfg.speculative.enabled {
        return;
    }
    let sentences = split_sentences(&hint.text);
    if sentences.is_empty() || is_control_marker(hint.text.trim()) {
        return;
    }
    let targets = dedupe_targets(
        inner
            .deps
            .targets
            .targets(&hint.room, &hint.speaker, &hint.lang)
            .await,
    );
    let terms = inner.deps.glossary.terms(&hint.room).await;
    let context = context_for(inner, &hint.room, &cfg).await;
    let ttl = Duration::from_millis(cfg.speculative.ttl_ms);
    let timeout = Duration::from_millis(cfg.speculative.timeout_ms);
    for lang in targets {
        if is_same_language(&hint.lang, &lang) {
            continue; // speculation is the one path that must never pay for an echo
        }
        for s in &sentences {
            let glossary = select_terms(s, &terms, cfg.glossary_limit);
            let repair = has_suspected_mishearing(&glossary) || !context.is_empty();
            let req = TranslateRequest {
                text: s.clone(),
                source_lang: hint.lang.clone(),
                target_lang: lang.clone(),
                glossary,
                context: context.clone(),
                repair,
            };
            let translator = inner.deps.translator.clone();
            inner.spec.try_prefetch(
                spec_key(&hint.room, &hint.speaker, &lang, s),
                ttl,
                timeout,
                async move { translator.translate(&req).await.ok() },
            );
        }
    }
}
