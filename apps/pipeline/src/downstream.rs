//! What happens to STT output. The room worker hands every [`SttSegment`] of a room to a
//! [`DownstreamStage`]: [`SttOnlyStage`] (captions + `stt:results`) or [`TranslateDubStage`]
//! (the full chain speech -> STT -> MT -> TTS -> dub published into the room).
//!
//! # `TranslateDubStage` latency design
//!
//! ```text
//! segments --> ingest --(early_mt: at once | else held until the turn is complete)--> MT stage
//!               |  caption + stt:results (after the forward, never before)               |
//!               |                                                                  Translation
//!               v                                                                        v
//!         room data channel                         translation task: -> TTS input FIRST,
//!                                                   then caption (+translations) and translate:results
//!                                                                                        v
//!                                  per (speaker, language) publisher <-- DubChunk <-- TTS stage
//!                                  (publish_dub per chunk, no buffering)
//! ```
//!
//! - Sub-stages are joined by bounded channels; only STT ever feels backpressure, never audio.
//! - A segment goes to MT before its caption is written, a translation goes to TTS before its
//!   caption is written: durable and visible output never sits on the critical path.
//! - Dub chunks are published the instant TTS yields them, one publisher task per
//!   (speaker, language) so a slow track of one language does not hold up another.
//! - Metrics, all relative to the last speech frame of the turn (0 when it came earlier):
//!   `e2e_first_caption`, `e2e_final_caption`, `e2e_first_translated_caption`,
//!   `e2e_first_dub_audio` (first dub chunk of the turn, mouth-to-ear proxy at the publish
//!   point), `e2e_last_dub_audio` (first audio of the turn's last sentence). The speech end of
//!   a turn is only known at commit, so they are computed when the end-of-turn marker arrives.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{debug, warn};
use uuid::Uuid;
use warptalk_contracts::events::TranslationRoomSystemEvent;
use warptalk_contracts::text::join_sentences;
use warptalk_kernel::RoomId;
use warptalk_mt::lang::is_same_language;
use warptalk_mt::text::normalize_key;
use warptalk_mt::{
    ContextProvider, GlossaryProvider, MtConfig, MtDeps, MtEvent, MtOutputs, NeverSkip,
    TargetLanguages, Translation, TranslationStage, Translator,
};
use warptalk_pipeline_runtime::SttSegment;
use warptalk_telemetry::{Outcome, record_stage_latency, record_stage_outcome};
use warptalk_tts::{
    CloneLadder, ConsentChecker, DubChunk, NoConsent, NoProfiles, Synthesizer, TtsCache, TtsConfig,
    TtsDeps, TtsError, TtsEvent, TtsOutputs, TtsStage, VoiceCatalogProvider, VoiceCloner,
    VoiceProfiles, VoiceResolver,
};

use crate::prewarm::PrewarmSynthesizer;
use crate::tuning::LatencyTuning;

use crate::results::{ResultSink, SttResultMeta, logprob_to_probability, stt_result_from_segment};
use crate::routes::RoutesView;
use crate::transport::{MediaSession, PcmChunk};

/// Time from segment arrival at the stage until its `stt:results` entry was written.
pub const STAGE_RESULT_PUBLISH: &str = "stt_result_publish";
/// First caption of a turn, relative to the last speech frame (0 when it beat the end of
/// speech, which is what flash mode is for; spec 08 section 9 question 1).
pub const STAGE_E2E_FIRST_CAPTION: &str = "e2e_first_caption";
/// Final (non-early) caption of a turn, relative to the last speech frame.
pub const STAGE_E2E_FINAL_CAPTION: &str = "e2e_final_caption";
/// First translated caption of a turn, relative to the last speech frame.
pub const STAGE_E2E_FIRST_TRANSLATED_CAPTION: &str = "e2e_first_translated_caption";
/// First dub audio chunk of a turn published into the room (mouth-to-ear proxy), relative to
/// the last speech frame.
pub const STAGE_E2E_FIRST_DUB: &str = "e2e_first_dub_audio";
/// First audio of the dub of the turn's LAST sentence, relative to the last speech frame: when
/// the listener has heard the end of what was said.
pub const STAGE_E2E_LAST_DUB: &str = "e2e_last_dub_audio";
/// Outcome counter label of the stt publish.
pub const STAGE_STT_PUBLISH: &str = "stt_publish";
/// Data-channel topic of caption messages.
pub const CAPTION_TOPIC: &str = warptalk_contracts::media::CAPTIONS_TOPIC;

/// Everything a stage needs for one room.
pub struct DownstreamContext {
    pub room: RoomId,
    /// Publish dub audio and data messages into the room.
    pub media: Arc<dyn MediaSession>,
    /// Durable output (`stt:results`).
    pub results: Arc<dyn ResultSink>,
    /// Current audio routes (speakers, target languages); changes as the room evolves.
    pub routes: watch::Receiver<Arc<RoutesView>>,
    /// Epoch milliseconds of the start of this room's translation session in this process: the
    /// origin of the `start_ms`/`end_ms` offsets on `stt:results` (also sent as `anchor_ms`).
    pub session_start_epoch_ms: i64,
}

impl DownstreamContext {
    /// Display name and user id of a speaker from the current routes.
    fn speaker_info(&self, speaker: &str) -> (Option<String>, Option<String>) {
        let v = self.routes.borrow();
        (
            v.name_of(speaker).map(str::to_owned),
            v.users.get(speaker).cloned(),
        )
    }
}

/// Consumes the segments of one room until the channel closes (room shutdown), then returns.
/// Per speaker, segments arrive in `seq` order.
#[async_trait]
pub trait DownstreamStage: Send + Sync {
    async fn run(&self, ctx: DownstreamContext, segments: mpsc::Receiver<SttSegment>);
}

/// Default stage: publish STT results to the result sink and the original-language caption to
/// the room data channel. No translation or dubbing.
#[derive(Debug, Default, Clone, Copy)]
pub struct SttOnlyStage;

#[async_trait]
impl DownstreamStage for SttOnlyStage {
    async fn run(&self, ctx: DownstreamContext, mut segments: mpsc::Receiver<SttSegment>) {
        // speaker -> (utterance_seq, first caption recorded)
        let mut first: HashMap<String, (u64, bool)> = HashMap::new();
        while let Some(seg) = segments.recv().await {
            if seg.is_partial {
                publish_partial_caption(&ctx, &seg).await;
                continue;
            }
            handle_segment(&ctx, &seg, &mut first).await;
        }
    }
}

async fn handle_segment(
    ctx: &DownstreamContext,
    seg: &SttSegment,
    first: &mut HashMap<String, (u64, bool)>,
) {
    let Some(published) = publish_stt_caption(ctx, seg).await else {
        return;
    };
    let since_end = |end: Option<Instant>| {
        end.map_or(0.0, |e| {
            published.saturating_duration_since(e).as_secs_f64() * 1000.0
        })
    };
    let entry = first
        .entry(seg.speaker.clone())
        .or_insert((seg.utterance_seq, false));
    if entry.0 != seg.utterance_seq {
        *entry = (seg.utterance_seq, false);
    }
    if !entry.1 {
        entry.1 = true;
        record_stage_latency(STAGE_E2E_FIRST_CAPTION, since_end(seg.stamps.speech_end));
    }
    if !seg.is_early {
        record_stage_latency(STAGE_E2E_FINAL_CAPTION, since_end(seg.stamps.speech_end));
    }
}

fn epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Live (uncommitted) text of a speaker: caption only, never stored or translated. One line per
/// speaker and segment counter (`live-<speaker>-<seq>`), replaced in place as it grows; clients
/// drop it when the speaker's next real `stt` caption arrives.
async fn publish_partial_caption(ctx: &DownstreamContext, seg: &SttSegment) {
    let text = seg.text.trim();
    if text.is_empty() {
        return;
    }
    let (speaker_name, speaker_user) = ctx.speaker_info(&seg.speaker);
    let caption = serde_json::json!({
        "type": "stt",
        "segment_id": format!("live-{}-{}", seg.speaker, seg.seq),
        "speaker_id": seg.speaker,
        "speakerId": seg.speaker,
        "speakerName": speaker_name,
        "speakerUserId": speaker_user,
        "utterance_seq": seg.utterance_seq,
        "text": text,
        "lang": (!seg.lang.is_empty()).then_some(seg.lang.as_str()),
        "partial": true,
        "final": false,
        "translations": {},
        "emittedAtMs": epoch_ms(),
    });
    if let Err(e) = ctx
        .media
        .publish_data(CAPTION_TOPIC, caption.to_string().into_bytes())
        .await
    {
        debug!(room = %ctx.room, speaker = %seg.speaker, error = %e, "partial caption publish failed");
    }
}

/// Writes the `stt:results` entry and the original-language caption (concurrently). The caption
/// starts with an empty `translations` map; translated captions follow as `"translation"`
/// messages with the same `segment_id`. Returns when both are done (`None`: empty text).
async fn publish_stt_caption(ctx: &DownstreamContext, seg: &SttSegment) -> Option<Instant> {
    let received = Instant::now();
    let (speaker_name, speaker_user) = ctx.speaker_info(&seg.speaker);
    let meta = SttResultMeta {
        speaker_name: speaker_name.as_deref(),
        session_start_epoch_ms: ctx.session_start_epoch_ms,
    };
    let result = stt_result_from_segment(ctx.room, seg, received, epoch_ms(), &meta)?;
    let caption = serde_json::json!({
        "type": "stt",
        "segment_id": result.segment_id,
        "meeting_id": result.meeting_id,
        "speaker_id": result.speaker_id,
        "speakerId": result.speaker_id,
        "speakerName": speaker_name,
        "speakerUserId": speaker_user,
        "seq": seg.seq,
        "utterance_seq": seg.utterance_seq,
        "text": result.text,
        "lang": result.language,
        // Flash sentences are final, append-only text: `is_early` stays false for clients
        // (the old meaning "interim forever" made every flash sentence render as interim).
        "is_early": false,
        "is_flash": seg.is_early,
        "translations": {},
        "ts_ms": result.timestamp_ms,
        "emittedAtMs": epoch_ms(),
    });
    // Caption and durable write are independent: neither waits for the other.
    let (stored, shown) = tokio::join!(
        async {
            let r = ctx.results.publish_stt(&result).await;
            record_stage_latency(
                STAGE_RESULT_PUBLISH,
                received.elapsed().as_secs_f64() * 1000.0,
            );
            r
        },
        ctx.media
            .publish_data(CAPTION_TOPIC, caption.to_string().into_bytes()),
    );
    match stored {
        Ok(()) => record_stage_outcome(STAGE_STT_PUBLISH, Outcome::Ok),
        Err(e) => {
            warn!(room = %ctx.room, speaker = %seg.speaker, error = %e, "stt:results publish failed");
            record_stage_outcome(STAGE_STT_PUBLISH, Outcome::Error);
        }
    }
    if let Err(e) = shown {
        warn!(room = %ctx.room, speaker = %seg.speaker, error = %e, "caption publish failed");
    }
    Some(Instant::now())
}

// ---------------------------------------------------------------------------------------------
// TranslateDubStage
// ---------------------------------------------------------------------------------------------

/// How long held early sentences wait for the end of their turn before they are translated
/// anyway (only when `early_mt` is off and the STT never reports completion).
const HOLD_WATCHDOG: Duration = Duration::from_secs(3);

/// Shared providers of every room the stage serves. The per-room sub-stages are created in
/// [`DownstreamStage::run`]; the watches make MT/TTS settings and the latency flags hot-reload.
pub struct TranslateDubDeps {
    pub translator: Arc<dyn Translator>,
    pub glossary: Arc<dyn GlossaryProvider>,
    pub context: Arc<dyn ContextProvider>,
    /// Wraps the vendor synthesizer; the stage pre-opens contexts through it (`tts_prewarm`).
    pub synth: Arc<PrewarmSynthesizer>,
    /// `false`: translate and caption only (no TTS provider configured).
    pub dub: bool,
    pub tts_cache: Arc<dyn TtsCache>,
    pub voices: Arc<VoiceResolver>,
    pub mt_cfg: watch::Receiver<MtConfig>,
    pub tts_cfg: watch::Receiver<TtsConfig>,
    pub tuning: watch::Receiver<LatencyTuning>,
}

/// speech -> STT -> MT -> TTS -> dub, per room. See the module docs for the data flow.
pub struct TranslateDubStage {
    deps: Arc<TranslateDubDeps>,
}

/// Stands in when no TTS provider is configured; never reached because `dub` is off.
struct NoDubSynthesizer;

#[async_trait]
impl Synthesizer for NoDubSynthesizer {
    fn sample_rate(&self) -> u32 {
        24_000
    }

    async fn open_context(
        &self,
        _p: &warptalk_tts::ContextParams,
    ) -> Result<Box<dyn warptalk_tts::SynthContext>, warptalk_tts::TtsError> {
        Err(warptalk_tts::TtsError::Permanent("no tts provider".into()))
    }
}

/// The vendor-facing pieces of the chain (real or fake).
pub struct Providers {
    pub translator: Arc<dyn Translator>,
    pub glossary: Arc<dyn GlossaryProvider>,
    pub context: Arc<dyn ContextProvider>,
    /// `None`: no TTS provider; the chain then stops at translated captions.
    pub synthesizer: Option<Arc<dyn Synthesizer>>,
    pub catalog: Arc<dyn VoiceCatalogProvider>,
    pub cloner: Arc<dyn VoiceCloner>,
    pub tts_cache: Arc<dyn TtsCache>,
}

impl TranslateDubStage {
    pub fn new(deps: TranslateDubDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }

    /// Wires the providers: the synthesizer gets the pre-warm decorator, voices resolve through
    /// the standard (non-cloned) path unless a consent checker is added later. Returns the
    /// decorator too (its stats show how often the pre-warm paid off).
    pub fn assemble(
        p: Providers,
        mt_cfg: watch::Receiver<MtConfig>,
        tts_cfg: watch::Receiver<TtsConfig>,
        tuning: watch::Receiver<LatencyTuning>,
    ) -> (Self, Arc<PrewarmSynthesizer>) {
        Self::assemble_with_voice(
            p,
            mt_cfg,
            tts_cfg,
            tuning,
            Arc::new(NoConsent),
            Arc::new(NoProfiles),
        )
    }

    /// [`Self::assemble`] with the runtime consent check and persisted voice profiles.
    pub fn assemble_with_voice(
        p: Providers,
        mt_cfg: watch::Receiver<MtConfig>,
        tts_cfg: watch::Receiver<TtsConfig>,
        tuning: watch::Receiver<LatencyTuning>,
        consent: Arc<dyn ConsentChecker>,
        profiles: Arc<dyn VoiceProfiles>,
    ) -> (Self, Arc<PrewarmSynthesizer>) {
        let dub = p.synthesizer.is_some();
        let synth = PrewarmSynthesizer::new(
            p.synthesizer
                .unwrap_or_else(|| Arc::new(NoDubSynthesizer) as Arc<dyn Synthesizer>),
        );
        let voices = Arc::new(VoiceResolver {
            cfg: tts_cfg.clone(),
            consent,
            ladder: CloneLadder::new(tts_cfg.clone(), p.cloner),
            profiles,
            catalog: p.catalog,
        });
        let stage = Self::new(TranslateDubDeps {
            translator: p.translator,
            glossary: p.glossary,
            context: p.context,
            synth: synth.clone(),
            dub,
            tts_cache: p.tts_cache,
            voices,
            mt_cfg,
            tts_cfg,
            tuning,
        });
        (stage, synth)
    }
}

/// Target languages of a speaker: the languages of the speaker's live routes minus the
/// speaker's own (those listeners hear the original; the caption path covers them).
fn targets_for(view: &RoutesView, speaker: &str, source_lang: &str) -> Vec<String> {
    view.speakers
        .get(speaker)
        .map(|s| {
            s.target_languages
                .iter()
                .filter(|l| !is_same_language(source_lang, l))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Consent and voice profiles are stored per USER (`voice:consent:{userId}`) while a pipeline
/// speaker is a participant: resolve through the room's current routes (the speaker itself when
/// the snapshot carries no user id).
struct UserKeyed<T: ?Sized> {
    inner: Arc<T>,
    routes: watch::Receiver<Arc<RoutesView>>,
}

impl<T: ?Sized> UserKeyed<T> {
    fn user(&self, speaker: &str) -> String {
        self.routes.borrow().user_of(speaker).to_owned()
    }
}

#[async_trait]
impl ConsentChecker for UserKeyed<dyn ConsentChecker> {
    async fn voice_clone_consented(&self, room: &str, speaker: &str) -> Result<bool, TtsError> {
        self.inner
            .voice_clone_consented(room, &self.user(speaker))
            .await
    }
}

#[async_trait]
impl VoiceProfiles for UserKeyed<dyn VoiceProfiles> {
    async fn carried_clone(&self, room: &str, speaker: &str) -> Option<String> {
        self.inner.carried_clone(room, &self.user(speaker)).await
    }

    async fn chosen_dub_voice(&self, room: &str, speaker: &str) -> Option<String> {
        self.inner.chosen_dub_voice(room, &self.user(speaker)).await
    }
}

/// The shared resolver with consent / profile lookups keyed by user for one room.
fn room_voices(v: &VoiceResolver, routes: &watch::Receiver<Arc<RoutesView>>) -> Arc<VoiceResolver> {
    Arc::new(VoiceResolver {
        cfg: v.cfg.clone(),
        consent: Arc::new(UserKeyed {
            inner: v.consent.clone(),
            routes: routes.clone(),
        }),
        ladder: v.ladder.clone(),
        profiles: Arc::new(UserKeyed {
            inner: v.profiles.clone(),
            routes: routes.clone(),
        }),
        catalog: v.catalog.clone(),
    })
}

/// MT's port over the room's routes watch: always the current listener languages.
struct RoutesTargets {
    routes: watch::Receiver<Arc<RoutesView>>,
}

#[async_trait]
impl TargetLanguages for RoutesTargets {
    async fn targets(&self, _room: &str, speaker: &str, source_lang: &str) -> Vec<String> {
        targets_for(&self.routes.borrow(), speaker, source_lang)
    }
}

#[derive(Default)]
struct TurnE2e {
    speech_end: Option<Instant>,
    first_caption: Option<Instant>,
    last_caption: Option<Instant>,
    first_translated: Option<Instant>,
    first_dub: Option<Instant>,
    /// Last segment handed to MT in this turn (final once `ended`).
    last_seg: Option<Uuid>,
    /// First dub audio per (segment, sentence, language).
    seg_first_dub: HashMap<(Uuid, u32, String), Instant>,
    /// Index of the final sentence of a segment, once its translation is known.
    final_chunk: HashMap<Uuid, u32>,
    segs: Vec<Uuid>,
    ended: bool,
    rec_first_caption: bool,
    rec_final_caption: bool,
    rec_first_translated: bool,
    rec_first_dub: bool,
    rec_last: HashSet<String>,
}

impl TurnE2e {
    /// Records every metric whose two ends are known. Idempotent.
    fn flush(&mut self) {
        let Some(end) = self.speech_end else { return };
        let ms = |at: Instant| at.saturating_duration_since(end).as_secs_f64() * 1000.0;
        if !self.rec_first_caption
            && let Some(at) = self.first_caption
        {
            self.rec_first_caption = true;
            record_stage_latency(STAGE_E2E_FIRST_CAPTION, ms(at));
        }
        if self.ended
            && !self.rec_final_caption
            && let Some(at) = self.last_caption
        {
            self.rec_final_caption = true;
            record_stage_latency(STAGE_E2E_FINAL_CAPTION, ms(at));
        }
        if !self.rec_first_translated
            && let Some(at) = self.first_translated
        {
            self.rec_first_translated = true;
            record_stage_latency(STAGE_E2E_FIRST_TRANSLATED_CAPTION, ms(at));
        }
        if !self.rec_first_dub
            && let Some(at) = self.first_dub
        {
            self.rec_first_dub = true;
            record_stage_latency(STAGE_E2E_FIRST_DUB, ms(at));
        }
        if self.ended
            && let Some(last) = self.last_seg
            && let Some(&chunk) = self.final_chunk.get(&last)
        {
            for ((seg, idx, lang), at) in &self.seg_first_dub {
                if *seg == last && *idx == chunk && self.rec_last.insert(lang.clone()) {
                    record_stage_latency(STAGE_E2E_LAST_DUB, ms(*at));
                }
            }
        }
    }
}

#[derive(Default)]
struct E2eInner {
    turns: HashMap<(String, u64), TurnE2e>,
    seg_turn: HashMap<Uuid, (String, u64)>,
}

/// Per-turn end-to-end latency bookkeeping of one room (see the module docs).
#[derive(Default)]
struct E2eTracker(Mutex<E2eInner>);

/// Turns kept per speaker; older ones are dropped when a turn ends.
const E2E_KEEP_TURNS: u64 = 8;

impl E2eTracker {
    fn with_turn(&self, speaker: &str, utt: u64, f: impl FnOnce(&mut TurnE2e)) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let t = g.turns.entry((speaker.to_string(), utt)).or_default();
        f(t);
        t.flush();
    }

    /// An STT segment arrived (any kind): remembers which turn its id belongs to.
    fn segment(&self, seg: &SttSegment) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        g.seg_turn
            .insert(seg.segment_id, (seg.speaker.clone(), seg.utterance_seq));
        let t = g
            .turns
            .entry((seg.speaker.clone(), seg.utterance_seq))
            .or_default();
        t.segs.push(seg.segment_id);
        if seg.stamps.speech_end.is_some() {
            t.speech_end = seg.stamps.speech_end;
        }
        t.flush();
    }

    fn forwarded(&self, seg: &SttSegment) {
        self.with_turn(&seg.speaker, seg.utterance_seq, |t| {
            t.last_seg = Some(seg.segment_id);
        });
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        g.seg_turn
            .insert(seg.segment_id, (seg.speaker.clone(), seg.utterance_seq));
    }

    fn caption(&self, speaker: &str, utt: u64, at: Instant) {
        self.with_turn(speaker, utt, |t| {
            t.first_caption.get_or_insert(at);
            t.last_caption = Some(at);
        });
    }

    fn translated_caption(&self, tr: &Translation, at: Instant) {
        self.with_turn(&tr.speaker, tr.utterance_seq, |t| {
            t.first_translated.get_or_insert(at);
            if tr.is_final_chunk {
                t.final_chunk.insert(tr.segment_id, tr.chunk_idx);
            }
        });
    }

    fn dub(&self, segment: Uuid, chunk_idx: u32, lang: &str, at: Instant) {
        let key = {
            let g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            g.seg_turn.get(&segment).cloned()
        };
        if let Some((speaker, utt)) = key {
            self.with_turn(&speaker, utt, |t| {
                t.first_dub.get_or_insert(at);
                t.seg_first_dub
                    .entry((segment, chunk_idx, lang.to_string()))
                    .or_insert(at);
            });
        }
    }

    fn turn_ended(&self, speaker: &str, utt: u64, speech_end: Option<Instant>) {
        self.with_turn(speaker, utt, |t| {
            if speech_end.is_some() {
                t.speech_end = speech_end;
            }
            t.ended = true;
        });
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let old: Vec<(String, u64)> = g
            .turns
            .keys()
            .filter(|(s, u)| s == speaker && u + E2E_KEEP_TURNS < utt)
            .cloned()
            .collect();
        for k in old {
            if let Some(t) = g.turns.remove(&k) {
                for id in t.segs {
                    g.seg_turn.remove(&id);
                }
            }
        }
    }
}

/// Early sentences of one turn waiting for its end (`early_mt` off).
struct Held {
    utt: u64,
    since: Instant,
    segs: Vec<SttSegment>,
}

/// One segment standing for everything in `segs` (never empty): joined text, the stamps and
/// identity of the last one, not early.
fn combine(mut segs: Vec<SttSegment>) -> Option<SttSegment> {
    let mut last = segs.pop()?;
    if !segs.is_empty() {
        let mut text = segs
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        text.push(' ');
        text.push_str(last.text.trim());
        last.text = text;
    }
    last.is_early = false;
    Some(last)
}

struct Ingest {
    ctx: Arc<DownstreamContext>,
    deps: Arc<TranslateDubDeps>,
    /// Voice resolver of this room (consent keyed by user).
    voices: Arc<VoiceResolver>,
    mt_tx: mpsc::Sender<SttSegment>,
    tracker: Arc<E2eTracker>,
    held: HashMap<String, Held>,
    /// speaker -> (utterance_seq, normalized texts already handed to MT in it).
    sent: HashMap<String, (u64, HashSet<String>)>,
}

impl Ingest {
    async fn run(mut self, mut segments: mpsc::Receiver<SttSegment>) {
        loop {
            // Wake when the oldest held turn reaches the watchdog, however busy other speakers are.
            let deadline = self.held.values().map(|h| h.since + HOLD_WATCHDOG).min();
            let seg = tokio::select! {
                s = segments.recv() => match s {
                    Some(s) => s,
                    None => break,
                },
                () = async {
                    match deadline {
                        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.flush_stale().await;
                    continue;
                }
            };
            self.on_segment(seg).await;
        }
        // Room shutdown: whatever is still held is translated now.
        let speakers: Vec<String> = self.held.keys().cloned().collect();
        for s in speakers {
            self.flush(&s, None).await;
        }
    }

    async fn on_segment(&mut self, seg: SttSegment) {
        if seg.is_partial {
            publish_partial_caption(&self.ctx, &seg).await;
            return;
        }
        if seg.utterance_end {
            self.tracker
                .turn_ended(&seg.speaker, seg.utterance_seq, seg.stamps.speech_end);
            self.flush(&seg.speaker, Some(&seg)).await;
            return;
        }
        if seg.text.trim().is_empty() {
            return;
        }
        self.tracker.segment(&seg);
        if self
            .held
            .get(&seg.speaker)
            .is_some_and(|h| h.utt != seg.utterance_seq)
        {
            self.flush(&seg.speaker, None).await;
        }
        let early_mt = self.deps.tuning.borrow().early_mt;
        if early_mt {
            self.forward(seg.clone()).await;
        } else if seg.is_early {
            self.held
                .entry(seg.speaker.clone())
                .or_insert_with(|| Held {
                    utt: seg.utterance_seq,
                    since: Instant::now(),
                    segs: Vec::new(),
                })
                .segs
                .push(seg.clone());
        } else {
            // The remainder completes the turn: everything held goes out with it.
            let mut segs = self
                .held
                .remove(&seg.speaker)
                .map(|h| h.segs)
                .unwrap_or_default();
            segs.push(seg.clone());
            if let Some(c) = combine(segs) {
                self.forward(c).await;
            }
        }
        // Visible and durable output after the critical-path hand-off.
        if let Some(at) = publish_stt_caption(&self.ctx, &seg).await {
            self.tracker.caption(&seg.speaker, seg.utterance_seq, at);
        }
    }

    /// Translates what is held for `speaker` as one segment. `marker` supplies the final stamps.
    async fn flush(&mut self, speaker: &str, marker: Option<&SttSegment>) {
        let Some(h) = self.held.remove(speaker) else {
            return;
        };
        if let Some(mut c) = combine(h.segs) {
            if let Some(m) = marker.filter(|m| m.utterance_seq == c.utterance_seq) {
                c.stamps = m.stamps;
            }
            self.forward(c).await;
        }
    }

    async fn flush_stale(&mut self) {
        let stale: Vec<String> = self
            .held
            .iter()
            .filter(|(_, h)| h.since.elapsed() >= HOLD_WATCHDOG)
            .map(|(s, _)| s.clone())
            .collect();
        for s in stale {
            warn!(speaker = %s, "turn end marker missing; translating held early sentences");
            self.flush(&s, None).await;
        }
    }

    /// Hands a segment to MT unless the same text of the same turn already went (an STT
    /// remainder repeating an early sentence must not be translated and dubbed twice).
    async fn forward(&mut self, seg: SttSegment) {
        let norm = normalize_key(&seg.text);
        let entry = self
            .sent
            .entry(seg.speaker.clone())
            .or_insert_with(|| (seg.utterance_seq, HashSet::new()));
        if entry.0 != seg.utterance_seq {
            *entry = (seg.utterance_seq, HashSet::new());
        }
        if !entry.1.insert(norm) {
            debug!(speaker = %seg.speaker, "duplicate sentence in turn not translated again");
            return;
        }
        self.tracker.forwarded(&seg);
        let tuning = self.deps.tuning.borrow().clone();
        if tuning.tts_prewarm && self.deps.dub {
            self.prewarm(&seg, Duration::from_millis(tuning.tts_prewarm_max_age_ms));
        }
        if self.mt_tx.send(seg).await.is_err() {
            warn!("translation stage gone; segment dropped");
        }
    }

    /// Opens the TTS contexts of the target languages in the background, so a cold dial runs
    /// while MT translates.
    fn prewarm(&self, seg: &SttSegment, max_age: Duration) {
        let targets = targets_for(&self.ctx.routes.borrow(), &seg.speaker, &seg.lang);
        if targets.is_empty() {
            return;
        }
        let (voices, synth) = (self.voices.clone(), self.deps.synth.clone());
        let (room, speaker) = (seg.room.clone(), seg.speaker.clone());
        tokio::spawn(async move {
            for lang in targets {
                let voice = voices.standard(&room, &speaker, &lang).await;
                synth.prewarm(&lang, &voice.voice_id, max_age);
            }
        });
    }
}

/// Sentences of one (segment, target language) seen so far, by sentence index.
#[derive(Default)]
struct SentencesSoFar {
    source: BTreeMap<u32, String>,
    target: BTreeMap<u32, String>,
}

/// Bound on segments being assembled (an abandoned segment never sends its last sentence).
const MAX_OPEN_SEGMENTS: usize = 1024;

/// Accumulates per-sentence translations into the cumulative text of their segment, so a client
/// that keys captions by `segment_id` shows the whole segment, not the last sentence.
#[derive(Default)]
struct SegmentAssembler {
    open: HashMap<(Uuid, String), SentencesSoFar>,
}

impl SegmentAssembler {
    /// Records `t` and returns the cumulative (source, target) text of its segment so far.
    fn add(&mut self, t: &Translation) -> (String, String) {
        if self.open.len() >= MAX_OPEN_SEGMENTS {
            self.open.clear();
        }
        let key = (t.segment_id, t.target_lang.clone());
        let e = self.open.entry(key.clone()).or_default();
        e.source.insert(t.chunk_idx, t.source_text.clone());
        e.target.insert(t.chunk_idx, t.text.clone());
        let source = join_sentences(&t.source_lang, e.source.values().map(String::as_str));
        let target = join_sentences(&t.target_lang, e.target.values().map(String::as_str));
        if t.is_final_chunk {
            self.open.remove(&key);
        }
        (source, target)
    }
}

/// The caption message of one translated sentence (`type: "translation"`). `translations` holds
/// the cumulative translation of the segment so far, `text` the cumulative source text;
/// `sentenceIndex`/`isLastSentence` say which sentence this is, `sentenceText` and
/// `sentenceTranslations` carry that sentence alone.
fn translation_caption(
    room: RoomId,
    t: &Translation,
    source: &str,
    target: &str,
    speaker_name: Option<&str>,
    speaker_user: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "type": "translation",
        "segment_id": t.segment_id,
        "wire_segment_id": t.wire_segment_id(),
        "chunk_idx": t.chunk_idx,
        "is_final_chunk": t.is_final_chunk,
        "sentenceIndex": t.chunk_idx,
        "isLastSentence": t.is_final_chunk,
        "meeting_id": room,
        "speaker_id": t.speaker,
        "speakerId": t.speaker,
        "speakerName": speaker_name,
        "speakerUserId": speaker_user,
        "seq": t.seq,
        "utterance_seq": t.utterance_seq,
        "text": source,
        "sentenceText": t.source_text,
        "lang": t.source_lang,
        "is_early": false,
        "is_flash": t.is_early,
        "translations": { t.target_lang.clone(): target },
        "sentenceTranslations": { t.target_lang.clone(): t.text.clone() },
        "emittedAtMs": epoch_ms(),
    })
}

async fn translation_task(
    ctx: Arc<DownstreamContext>,
    mut rx: mpsc::Receiver<Translation>,
    tts_tx: mpsc::Sender<Translation>,
    dub: bool,
    tracker: Arc<E2eTracker>,
) {
    let mut assembler = SegmentAssembler::default();
    while let Some(t) = rx.recv().await {
        // TTS first: the dub must not wait for a caption or a stream write.
        if dub && !t.passthrough && tts_tx.send(t.clone()).await.is_err() {
            warn!("tts stage gone; translation not dubbed");
        }
        let (source, target) = assembler.add(&t);
        let (name, user) = ctx.speaker_info(&t.speaker);
        let caption = translation_caption(
            ctx.room,
            &t,
            &source,
            &target,
            name.as_deref(),
            user.as_deref(),
        );
        let mut contract = t.to_contract();
        // The wire confidence is a probability; the runtime's is an average logprob.
        contract.source_stt_confidence = t.confidence.and_then(logprob_to_probability);
        let (stored, shown) = tokio::join!(
            ctx.results.publish_translate(&contract),
            ctx.media
                .publish_data(CAPTION_TOPIC, caption.to_string().into_bytes()),
        );
        if let Err(e) = stored {
            warn!(room = %ctx.room, error = %e, "translate:results publish failed");
        }
        if let Err(e) = shown {
            warn!(room = %ctx.room, error = %e, "translated caption publish failed");
        }
        tracker.translated_caption(&t, Instant::now());
    }
}

/// Routes dub chunks to one publisher task per (speaker, language); chunks go out the moment
/// they arrive, in order per key.
async fn dub_router(
    ctx: Arc<DownstreamContext>,
    mut rx: mpsc::Receiver<DubChunk>,
    tracker: Arc<E2eTracker>,
    capacity: usize,
) {
    let mut pubs: HashMap<(String, String), mpsc::Sender<DubChunk>> = HashMap::new();
    let mut tasks: JoinSet<()> = JoinSet::new();
    while let Some(chunk) = rx.recv().await {
        if chunk.pcm.is_empty() {
            continue; // end-of-sentence marker: nothing to play
        }
        let key = (chunk.speaker.clone(), chunk.target_lang.clone());
        let tx = pubs
            .entry(key)
            .or_insert_with(|| {
                let (tx, prx) = mpsc::channel(capacity);
                tasks.spawn(dub_publisher(ctx.clone(), prx, tracker.clone()));
                tx
            })
            .clone();
        let _ = tx.send(chunk).await;
    }
    drop(pubs);
    while tasks.join_next().await.is_some() {}
}

async fn dub_publisher(
    ctx: Arc<DownstreamContext>,
    mut rx: mpsc::Receiver<DubChunk>,
    tracker: Arc<E2eTracker>,
) {
    while let Some(c) = rx.recv().await {
        let chunk = PcmChunk {
            samples: c.pcm.clone(),
            sample_rate_hz: c.sample_rate,
        };
        match ctx
            .media
            .publish_dub(&c.target_lang, &c.voice.voice_id, chunk)
            .await
        {
            Ok(()) => tracker.dub(c.segment_id, c.chunk_idx, &c.target_lang, Instant::now()),
            Err(e) => {
                warn!(room = %ctx.room, lang = %c.target_lang, error = %e, "dub publish failed")
            }
        }
    }
}

async fn event_task(
    ctx: Arc<DownstreamContext>,
    mut mt: mpsc::Receiver<MtEvent>,
    mut tts: mpsc::Receiver<TtsEvent>,
) {
    let (mut mt_open, mut tts_open) = (true, true);
    while mt_open || tts_open {
        tokio::select! {
            e = mt.recv(), if mt_open => match e {
                Some(MtEvent::Failed { speaker, target_lang, error, .. }) => {
                    warn!(room = %ctx.room, %speaker, %target_lang, %error, "translation failed");
                }
                Some(MtEvent::OrderWaitExceeded { speaker, target_lang, .. }) => {
                    warn!(room = %ctx.room, %speaker, %target_lang, "mt publish order wait exceeded");
                }
                Some(MtEvent::Skipped { reason, .. }) => debug!(room = %ctx.room, ?reason, "mt skipped"),
                None => mt_open = false,
            },
            e = tts.recv(), if tts_open => match e {
                Some(ev) => handle_tts_event(&ctx, ev).await,
                None => tts_open = false,
            },
        }
    }
}

async fn handle_tts_event(ctx: &DownstreamContext, ev: TtsEvent) {
    match &ev {
        TtsEvent::SentenceDone { .. } => {
            if let Some(r) = ev.to_contract()
                && let Err(e) = ctx.results.publish_tts(&r).await
            {
                warn!(room = %ctx.room, error = %e, "tts:results publish failed");
            }
        }
        TtsEvent::RouteDegraded {
            speaker,
            target_lang,
            from,
            to,
            reason,
            ..
        } => {
            let route_id = ctx
                .routes
                .borrow()
                .routes
                .iter()
                .find(|r| {
                    r.source_participant_id.to_string() == *speaker
                        && is_same_language(&r.target_language, target_lang)
                })
                .map(|r| r.route_id);
            let event = TranslationRoomSystemEvent {
                event_type: "ROUTE_DEGRADED".into(),
                route_id,
                room_id: ctx.room,
                payload: serde_json::json!({
                    "speaker_id": speaker,
                    "target_lang": target_lang,
                    "from": format!("{from:?}"),
                    "to": format!("{to:?}"),
                    "reason": reason,
                }),
            };
            warn!(room = %ctx.room, %speaker, %target_lang, ?from, ?to, %reason, "route degraded");
            if let Err(e) = ctx.results.publish_system_event(&event).await {
                warn!(room = %ctx.room, error = %e, "system event publish failed");
            }
        }
        TtsEvent::DeadLetter {
            speaker,
            target_lang,
            reason,
            error,
            ..
        } => {
            warn!(room = %ctx.room, %speaker, %target_lang, %reason, %error, "dub dead-lettered");
        }
    }
}

#[async_trait]
impl DownstreamStage for TranslateDubStage {
    async fn run(&self, ctx: DownstreamContext, segments: mpsc::Receiver<SttSegment>) {
        let ctx = Arc::new(ctx);
        let deps = &*self.deps;
        let cap = deps.tuning.borrow().stage_channel_capacity.max(1);
        let voices = room_voices(&deps.voices, &ctx.routes);
        let (mt_in_tx, mt_in_rx) = mpsc::channel(cap);
        let (tr_tx, tr_rx) = mpsc::channel(cap);
        let (mt_ev_tx, mt_ev_rx) = mpsc::channel(256);
        let (tts_in_tx, tts_in_rx) = mpsc::channel(cap);
        let (chunk_tx, chunk_rx) = mpsc::channel(cap.max(32));
        let (tts_ev_tx, tts_ev_rx) = mpsc::channel(256);

        let mt = TranslationStage::spawn(
            MtDeps {
                translator: deps.translator.clone(),
                glossary: deps.glossary.clone(),
                targets: Arc::new(RoutesTargets {
                    routes: ctx.routes.clone(),
                }),
                skip: Arc::new(NeverSkip),
                context: deps.context.clone(),
            },
            deps.mt_cfg.clone(),
            mt_in_rx,
            None,
            MtOutputs {
                translations: tr_tx,
                events: Some(mt_ev_tx),
            },
        );
        let tts = TtsStage::spawn(
            TtsDeps {
                synth: deps.synth.clone(),
                cache: deps.tts_cache.clone(),
                voices: voices.clone(),
            },
            deps.tts_cfg.clone(),
            tts_in_rx,
            TtsOutputs {
                chunks: chunk_tx,
                events: Some(tts_ev_tx),
            },
        );

        let tracker = Arc::new(E2eTracker::default());
        let translations = tokio::spawn(translation_task(
            ctx.clone(),
            tr_rx,
            tts_in_tx,
            deps.dub,
            tracker.clone(),
        ));
        let dubs = tokio::spawn(dub_router(ctx.clone(), chunk_rx, tracker.clone(), cap));
        let events = tokio::spawn(event_task(ctx.clone(), mt_ev_rx, tts_ev_rx));

        Ingest {
            ctx: ctx.clone(),
            deps: self.deps.clone(),
            voices,
            mt_tx: mt_in_tx,
            tracker,
            held: HashMap::new(),
            sent: HashMap::new(),
        }
        .run(segments)
        .await;

        // Ingest dropped its MT sender: drain MT, then TTS, then the publishers.
        mt.join().await;
        let _ = translations.await;
        tts.join().await;
        let _ = dubs.await;
        let _ = events.await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod user_keyed_tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl ConsentChecker for Recorder {
        async fn voice_clone_consented(
            &self,
            _room: &str,
            speaker: &str,
        ) -> Result<bool, TtsError> {
            self.0.lock().unwrap().push(speaker.to_owned());
            Ok(true)
        }
    }

    #[tokio::test]
    async fn consent_is_asked_for_the_user_not_the_participant() {
        use chrono::Utc;
        use warptalk_contracts::events::{AudioRoute, AudioRoutesSnapshot};
        use warptalk_kernel::{ParticipantId as Pid, UserId};
        let (pid, uid, other) = (Pid::new(), UserId::new(), Pid::new());
        let view = RoutesView::from_snapshot(&AudioRoutesSnapshot {
            generated_at: Utc::now(),
            room_status: "Active".into(),
            translation_active: true,
            room_languages: vec![],
            routes: vec![AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: pid,
                target_participant_id: Pid::new(),
                source_user_id: Some(uid),
                target_user_id: None,
                source_display_name: None,
                source_language: "en".into(),
                target_language: "vi".into(),
                voice_clone_enabled: true,
                status: warptalk_contracts::events::RouteStatus::Broadcasting,
            }],
        });
        let (_tx, rx) = watch::channel(Arc::new(view));
        let rec = Arc::new(Recorder::default());
        let keyed = UserKeyed::<dyn ConsentChecker> {
            inner: rec.clone(),
            routes: rx,
        };
        keyed
            .voice_clone_consented("r", &pid.to_string())
            .await
            .unwrap();
        // unknown speaker (no user id in the snapshot): used as is
        keyed
            .voice_clone_consented("r", &other.to_string())
            .await
            .unwrap();
        assert_eq!(
            *rec.0.lock().unwrap(),
            vec![uid.to_string(), other.to_string()]
        );
    }
}
