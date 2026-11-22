#![allow(clippy::unwrap_used)]
//! The full chain (SimulatedTransport -> RoomWorker -> JitterStt -> MT -> TTS -> dub) with the
//! provider doubles, plus scripted-segment tests of the stage's ordering and de-duplication.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use warptalk_contracts::events::{AudioRoute, AudioRoutesSnapshot, RouteStatus};
use warptalk_kernel::{ParticipantId, RoomId, UserId};
use warptalk_mt::{InMemoryGlossary, MtConfig, NoContext};
use warptalk_pipeline::downstream::{
    CAPTION_TOPIC, DownstreamContext, DownstreamStage, Providers, TranslateDubStage,
};
use warptalk_pipeline::fakes::{FakeMt, FakeMtConfig, FakeTts, FakeTtsConfig};
use warptalk_pipeline::transport::MediaTransport;
use warptalk_pipeline::tuning::LatencyTuning;
use warptalk_pipeline::{
    MemoryResultSink, RoomWorker, RoomWorkerDeps, RoutesView, SimParticipant, SimulatedTransport,
};
use warptalk_pipeline_runtime::testing::{JitterStt, JitterSttConfig, energy_vad_factory};
use warptalk_pipeline_runtime::{PipelineConfig, SttSegment};
use warptalk_tts::testing::{FakeCatalog, FakeCloner};
use warptalk_tts::{
    ContextParams, InMemoryTtsCache, SynthContext, Synthesizer, TtsConfig, TtsError,
};

fn view(speakers: &[ParticipantId], langs: &[&str]) -> RoutesView {
    RoutesView::from_snapshot(&AudioRoutesSnapshot {
        generated_at: Utc::now(),
        room_status: "Active".into(),
        translation_active: true,
        room_languages: std::iter::once("en")
            .chain(langs.iter().copied())
            .map(String::from)
            .collect(),
        routes: speakers
            .iter()
            .flat_map(|s| {
                langs.iter().map(|l| AudioRoute {
                    route_id: Uuid::now_v7(),
                    source_participant_id: *s,
                    target_participant_id: ParticipantId::new(),
                    source_user_id: None,
                    target_user_id: None,
                    source_display_name: None,
                    source_language: "en".into(),
                    target_language: (*l).into(),
                    voice_clone_enabled: false,
                    status: RouteStatus::Broadcasting,
                })
            })
            .collect(),
    })
}

/// Routes of `speaker` (user `user`, display name `name`) into `langs`.
fn named_view(speaker: ParticipantId, user: UserId, name: &str, langs: &[&str]) -> RoutesView {
    RoutesView::from_snapshot(&AudioRoutesSnapshot {
        generated_at: Utc::now(),
        room_status: "Active".into(),
        translation_active: true,
        room_languages: vec!["en".into()],
        routes: langs
            .iter()
            .map(|l| AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: speaker,
                target_participant_id: ParticipantId::new(),
                source_user_id: Some(user),
                target_user_id: None,
                source_display_name: Some(name.into()),
                source_language: "en".into(),
                target_language: (*l).into(),
                voice_clone_enabled: false,
                status: RouteStatus::Broadcasting,
            })
            .collect(),
    })
}

fn fast_mt() -> Arc<FakeMt> {
    FakeMt::new(FakeMtConfig {
        first_p50_ms: 30.0,
        first_p95_ms: 30.0,
        first_token_lead_ms: 5.0,
        batch_p50_ms: 30.0,
        batch_p95_ms: 30.0,
        seed: 1,
    })
}

fn fast_tts() -> Arc<FakeTts> {
    FakeTts::new(FakeTtsConfig {
        warm_p50_ms: 20.0,
        warm_p95_ms: 20.0,
        cold_probability: 0.0,
        ms_per_char: 8.0,
        realtime_factor: 8.0,
        ..FakeTtsConfig::default()
    })
}

struct Chain {
    stage: Arc<TranslateDubStage>,
    mt: Arc<FakeMt>,
    _keep: Vec<Box<dyn std::any::Any + Send + Sync>>,
}

fn chain_with(
    early_mt: bool,
    prewarm: bool,
    synth: Arc<dyn Synthesizer>,
    mt: Arc<FakeMt>,
) -> Chain {
    let (a, mt_rx) = watch::channel(MtConfig::default());
    let (b, tts_rx) = watch::channel(TtsConfig::default());
    let (c, tu_rx) = watch::channel(LatencyTuning {
        early_mt,
        tts_prewarm: prewarm,
        ..LatencyTuning::default()
    });
    let (stage, _) = TranslateDubStage::assemble(
        Providers {
            translator: mt.clone(),
            glossary: Arc::new(InMemoryGlossary::new()),
            context: Arc::new(NoContext),
            synthesizer: Some(synth),
            catalog: Arc::new(FakeCatalog(vec!["voice-a".into()])),
            cloner: Arc::new(FakeCloner::default()),
            tts_cache: Arc::new(InMemoryTtsCache::new(64)),
        },
        mt_rx,
        tts_rx,
        tu_rx,
    );
    Chain {
        stage: Arc::new(stage),
        mt,
        _keep: vec![Box::new((a, b, c))],
    }
}

async fn wait(cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---- full chain through the room worker ------------------------------------------------------

#[tokio::test]
async fn chain_round_trip_dubs_every_target_language_in_order() {
    let room = RoomId::new();
    let pid = ParticipantId::new();
    let transport = Arc::new(SimulatedTransport::with_participants(vec![
        SimParticipant::synthetic(pid.to_string(), 3000, 1500).utterances(1),
    ]));
    let results = Arc::new(MemoryResultSink::default());
    let chain = chain_with(true, true, fast_tts(), fast_mt());
    let stt = Arc::new(JitterStt::new(JitterSttConfig {
        sentence_ms: 1000,
        sentence_jitter: 0.0,
        lag_p50_ms: 80.0,
        lag_p95_ms: 80.0,
        tail_lag_p50_ms: 80.0,
        tail_lag_p95_ms: 80.0,
        commit_to_done_p50_ms: 100.0,
        commit_to_done_p95_ms: 100.0,
        ..JitterSttConfig::default()
    }));
    let (_tx, cfg) = watch::channel(PipelineConfig::default());
    let worker = RoomWorker::start(RoomWorkerDeps {
        room,
        transport: transport.clone(),
        stt: stt.clone(),
        vad_factory: energy_vad_factory(),
        downstream: chain.stage.clone(),
        results: results.clone(),
        config: cfg,
        flash_override: None,
        identity: "agent".into(),
        initial_routes: view(&[pid], &["vi", "ja"]),
    })
    .await
    .unwrap();
    wait(|| worker.speakers() == vec![pid.to_string()]).await;
    tokio::time::sleep(Duration::from_millis(5500)).await;
    worker.stop().await;
    assert!(worker.is_finished());

    let cap = transport.capture(room).unwrap();
    // Dub audio for each target language, voice id from the catalog.
    let dubs = cap.dubs();
    for lang in ["vi", "ja"] {
        assert!(
            dubs.iter().any(|d| d.target_lang == lang && d.samples > 0),
            "no dub published for {lang}"
        );
    }
    assert!(dubs.iter().all(|d| d.voice == "voice-a"));

    // Captions: the original with an empty translations map, then one translated caption per
    // sentence and language, each carrying emittedAtMs.
    let captions = cap.json_on(CAPTION_TOPIC);
    let stt_caps: Vec<_> = captions
        .iter()
        .filter(|(_, v)| v["type"] == "stt")
        .collect();
    assert!(stt_caps.len() >= 2, "early sentence + tail expected");
    assert!(stt_caps.iter().all(|(_, v)| {
        v["translations"].as_object().is_some_and(|m| m.is_empty()) && v["emittedAtMs"].is_i64()
    }));
    let tr_caps: Vec<_> = captions
        .iter()
        .filter(|(_, v)| v["type"] == "translation")
        .collect();
    for lang in ["vi", "ja"] {
        let mine: Vec<_> = tr_caps
            .iter()
            .filter(|(_, v)| v["translations"][lang].is_string())
            .collect();
        assert_eq!(
            mine.len(),
            stt_caps.len(),
            "one translated caption per sentence for {lang}"
        );
        assert!(mine.iter().all(|(_, v)| v["emittedAtMs"].is_i64()));
        assert!(mine.iter().all(|(_, v)| {
            v["translations"][lang]
                .as_str()
                .unwrap()
                .starts_with(&format!("[{lang}]"))
        }));
        // Ordering: translated captions of a language follow the order of the source sentences.
        let source: Vec<&str> = stt_caps
            .iter()
            .map(|(_, v)| v["text"].as_str().unwrap())
            .collect();
        let translated: Vec<&str> = mine
            .iter()
            .map(|(_, v)| v["text"].as_str().unwrap())
            .collect();
        assert_eq!(translated, source, "order preserved for {lang}");
    }

    // Durable metadata: translate:results / tts:results per language, same order.
    let tr = results.translations();
    let tts = results.tts_results();
    for lang in ["vi", "ja"] {
        let t: Vec<_> = tr
            .iter()
            .filter(|r| r.target_lang == lang)
            .map(|r| &r.segment_id)
            .collect();
        let d: Vec<_> = tts
            .iter()
            .filter(|r| r.target_lang == lang)
            .map(|r| &r.segment_id)
            .collect();
        assert_eq!(t.len(), stt_caps.len());
        assert_eq!(
            d, t,
            "tts:results follow translate:results order for {lang}"
        );
        assert!(t.iter().all(|id| id.contains(&format!("-{lang}-c"))));
    }
    assert!(
        tts.iter()
            .all(|r| r.provider_voice_id.as_deref() == Some("voice-a"))
    );
    assert!(results.system_events().is_empty());
}

// ---- scripted segments: ordering and duplicates ----------------------------------------------

struct Rig {
    tx: mpsc::Sender<SttSegment>,
    run: tokio::task::JoinHandle<()>,
    results: Arc<MemoryResultSink>,
    transport: Arc<SimulatedTransport>,
    room: RoomId,
    _routes: watch::Sender<Arc<RoutesView>>,
}

async fn rig(chain: &Chain, speaker: ParticipantId) -> Rig {
    rig_with_view(chain, view(&[speaker], &["vi"])).await
}

async fn rig_with_view(chain: &Chain, routes_view: RoutesView) -> Rig {
    let room = RoomId::new();
    let transport = Arc::new(SimulatedTransport::with_participants(vec![]));
    let session: Arc<dyn warptalk_pipeline::MediaSession> =
        Arc::from(transport.join(room, "agent").await.unwrap());
    let results = Arc::new(MemoryResultSink::default());
    let (routes_tx, routes) = watch::channel(Arc::new(routes_view));
    let (tx, rx) = mpsc::channel(32);
    let stage = chain.stage.clone();
    let ctx = DownstreamContext {
        room,
        media: session,
        results: results.clone(),
        routes,
        session_start_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    let run = tokio::spawn(async move { stage.run(ctx, rx).await });
    Rig {
        tx,
        run,
        results,
        transport,
        room,
        _routes: routes_tx,
    }
}

fn seg(speaker: ParticipantId, utt: u64, seq: u64, text: &str, early: bool) -> SttSegment {
    let mut s = SttSegment::new(text, "en", None, early);
    s.room = "r".into();
    s.speaker = speaker.to_string();
    s.seq = seq;
    s.utterance_seq = utt;
    s
}

fn marker(speaker: ParticipantId, utt: u64) -> SttSegment {
    let mut m = SttSegment::end_marker("en");
    m.speaker = speaker.to_string();
    m.utterance_seq = utt;
    m.stamps.speech_end = Some(Instant::now());
    m
}

#[tokio::test]
async fn early_pipelining_translates_each_sentence_once_and_in_order() {
    let chain = chain_with(true, false, fast_tts(), fast_mt());
    let pid = ParticipantId::new();
    let rig = rig(&chain, pid).await;
    // Two early sentences, the remainder REPEATS the second one (an STT that revised nothing
    // but re-sent it), then a new turn.
    for s in [
        seg(pid, 0, 0, "First sentence here.", true),
        seg(pid, 0, 1, "Second sentence here.", true),
        seg(pid, 0, 2, "Second sentence here.", false),
        seg(pid, 0, 2, "Third and last bit.", false),
        marker(pid, 0),
        seg(pid, 1, 3, "Second sentence here.", false),
        marker(pid, 1),
    ] {
        rig.tx.send(s).await.unwrap();
    }
    drop(rig.tx);
    rig.run.await.unwrap();

    let reqs = chain.mt.requests();
    let texts: Vec<&str> = reqs.iter().map(|(_, t)| t.as_str()).collect();
    // Turn 0: three distinct sentences once each; turn 1 may reuse a sentence of turn 0.
    assert_eq!(
        texts,
        vec![
            "First sentence here.",
            "Second sentence here.",
            "Third and last bit.",
            "Second sentence here."
        ],
        "each sentence of a turn is translated exactly once, in arrival order"
    );
    let order: Vec<String> = rig
        .results
        .translations()
        .into_iter()
        .map(|r| r.translated_text)
        .collect();
    assert_eq!(
        order,
        vec![
            "[vi] First sentence here.",
            "[vi] Second sentence here.",
            "[vi] Third and last bit.",
            "[vi] Second sentence here."
        ]
    );
    let _ = (rig.transport, rig.room);
}

#[tokio::test]
async fn without_early_mt_the_turn_is_translated_together_when_it_completes() {
    let mt = FakeMt::new(FakeMtConfig {
        first_p50_ms: 5.0,
        first_p95_ms: 5.0,
        batch_p50_ms: 5.0,
        batch_p95_ms: 5.0,
        ..FakeMtConfig::default()
    });
    let chain = chain_with(false, false, fast_tts(), mt);
    let pid = ParticipantId::new();
    let rig = rig(&chain, pid).await;
    rig.tx
        .send(seg(pid, 0, 0, "Alpha one.", true))
        .await
        .unwrap();
    rig.tx
        .send(seg(pid, 0, 1, "Beta two.", true))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        chain.mt.requests().is_empty(),
        "held until the turn is complete"
    );
    // Captions never wait: both original captions are already out.
    let cap = rig.transport.capture(rig.room).unwrap();
    assert_eq!(
        cap.json_on(CAPTION_TOPIC)
            .iter()
            .filter(|(_, v)| v["type"] == "stt")
            .count(),
        2
    );
    rig.tx.send(marker(pid, 0)).await.unwrap();
    drop(rig.tx);
    rig.run.await.unwrap();
    let texts: Vec<String> = chain.mt.requests().into_iter().map(|(_, t)| t).collect();
    assert_eq!(texts, vec!["Alpha one.", "Beta two."]);
}

// ---- degradation -------------------------------------------------------------------------------

struct RefusingTts;

#[async_trait]
impl Synthesizer for RefusingTts {
    fn sample_rate(&self) -> u32 {
        24_000
    }
    async fn open_context(&self, _p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError> {
        Err(TtsError::Permanent("voice refused".into()))
    }
}

#[tokio::test]
async fn tts_failure_degrades_the_route_but_captions_continue() {
    let chain = chain_with(true, false, Arc::new(RefusingTts), fast_mt());
    let pid = ParticipantId::new();
    let rig = rig(&chain, pid).await;
    rig.tx
        .send(seg(pid, 0, 0, "Hello there.", false))
        .await
        .unwrap();
    rig.tx.send(marker(pid, 0)).await.unwrap();
    drop(rig.tx);
    rig.run.await.unwrap();

    let events = rig.results.system_events();
    assert!(
        events.iter().any(|e| e.event_type == "ROUTE_DEGRADED"
            && e.payload["target_lang"] == "vi"
            && e.payload["to"] == "TextOnly"
            && e.route_id.is_some()),
        "events: {events:?}"
    );
    let cap = rig.transport.capture(rig.room).unwrap();
    assert!(cap.dubs().is_empty());
    assert_eq!(
        cap.json_on(CAPTION_TOPIC)
            .iter()
            .filter(|(_, v)| v["type"] == "translation")
            .count(),
        1,
        "the translated caption still goes out"
    );
}

// ---- wire contracts with the consumers (web, macOS, transcript) -----------------------------------

/// B-01: the dub goes out on `dub-{lang}`, the name the web (`pub.trackName === "dub-${listen}"`)
/// and macOS (`"dub-\(language)"`) subscribe to.
#[tokio::test]
async fn dub_is_published_on_the_track_clients_subscribe_to() {
    let chain = chain_with(true, false, fast_tts(), fast_mt());
    let pid = ParticipantId::new();
    let rig = rig(&chain, pid).await;
    rig.tx
        .send(seg(pid, 0, 0, "Hello there.", false))
        .await
        .unwrap();
    rig.tx.send(marker(pid, 0)).await.unwrap();
    drop(rig.tx);
    rig.run.await.unwrap();
    let dubs = rig.transport.capture(rig.room).unwrap().dubs();
    assert!(!dubs.is_empty());
    for d in &dubs {
        assert_eq!(d.target_lang, "vi");
        assert_eq!(
            d.track_name, "dub-vi",
            "literal the web and macOS compare against"
        );
    }
}

fn caption_of_type(
    cap: &warptalk_pipeline::simulated::SimCapture,
    ty: &str,
) -> Vec<serde_json::Value> {
    cap.json_on(CAPTION_TOPIC)
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v["type"] == ty)
        .collect()
}

/// B-03, B-06, R-02, R-03 on the stt side: speaker name on captions and results, offsets that
/// fit the transcript's i32 columns with a real epoch clock, probability confidence, flash
/// sentences final.
#[tokio::test]
async fn stt_captions_and_results_carry_name_offsets_confidence_and_finality() {
    let chain = chain_with(true, false, fast_tts(), fast_mt());
    let (pid, user) = (ParticipantId::new(), UserId::new());
    let rig = rig_with_view(&chain, named_view(pid, user, "Ann Lee", &["vi"])).await;
    let mut early = seg(pid, 0, 0, "A flash sentence.", true);
    early.confidence = Some(-0.5);
    rig.tx.send(early).await.unwrap();
    rig.tx.send(marker(pid, 0)).await.unwrap();
    drop(rig.tx);
    rig.run.await.unwrap();

    let results = rig.results.results();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.far_speaker_name.as_deref(), Some("Ann Lee"));
    assert_eq!(r.speaker_id, pid.to_string());
    assert!(
        r.is_final_chunk,
        "flash sentences are final, append-only text"
    );
    assert!(
        r.start_ms >= 0 && r.end_ms >= r.start_ms && r.end_ms < 60_000,
        "{r:?}"
    );
    assert!(
        r.anchor_ms.unwrap() > 1_000_000_000_000,
        "anchor is the epoch of the session start"
    );
    assert!(r.timestamp_ms.unwrap() >= r.anchor_ms.unwrap());
    assert!((r.confidence.unwrap() - (-0.5f64).exp()).abs() < 1e-6);

    let cap = rig.transport.capture(rig.room).unwrap();
    let stt = caption_of_type(&cap, "stt");
    assert_eq!(stt.len(), 1);
    assert_eq!(stt[0]["speakerName"], "Ann Lee");
    assert_eq!(stt[0]["speakerId"], pid.to_string());
    assert_eq!(stt[0]["speaker_id"], pid.to_string());
    assert_eq!(stt[0]["speakerUserId"], user.to_string());
    assert_eq!(
        stt[0]["is_early"], false,
        "a flash sentence renders as final"
    );
    assert_eq!(stt[0]["is_flash"], true);
    let tr = caption_of_type(&cap, "translation");
    assert_eq!(tr[0]["speakerName"], "Ann Lee");
    // the confidence of the translate result is a probability as well
    let t = &rig.results.translations()[0];
    assert!((t.source_stt_confidence.unwrap() - (-0.5f64).exp()).abs() < 1e-6);
}

/// B-04: one segment of three sentences makes three translation messages that carry the
/// sentence index, the last-sentence flag and the cumulative text of the segment so far.
#[tokio::test]
async fn multi_sentence_segment_captions_are_cumulative() {
    let chain = chain_with(true, false, fast_tts(), fast_mt());
    let pid = ParticipantId::new();
    let rig = rig(&chain, pid).await;
    let s = seg(
        pid,
        0,
        0,
        "One fine day. Two good ideas. Three more things.",
        false,
    );
    let segment_id = s.segment_id.to_string();
    rig.tx.send(s).await.unwrap();
    rig.tx.send(marker(pid, 0)).await.unwrap();
    drop(rig.tx);
    rig.run.await.unwrap();

    let cap = rig.transport.capture(rig.room).unwrap();
    let mut tr = caption_of_type(&cap, "translation");
    tr.sort_by_key(|v| v["sentenceIndex"].as_u64());
    assert_eq!(tr.len(), 3, "{tr:?}");
    for (i, v) in tr.iter().enumerate() {
        assert_eq!(
            v["segment_id"], segment_id,
            "one segment id for all sentences"
        );
        assert_eq!(v["sentenceIndex"], i);
        assert_eq!(v["isLastSentence"], i == 2);
    }
    assert_eq!(tr[0]["translations"]["vi"], "[vi] One fine day.");
    assert_eq!(
        tr[1]["translations"]["vi"],
        "[vi] One fine day. [vi] Two good ideas."
    );
    assert_eq!(
        tr[2]["translations"]["vi"],
        "[vi] One fine day. [vi] Two good ideas. [vi] Three more things."
    );
    assert_eq!(
        tr[2]["text"],
        "One fine day. Two good ideas. Three more things."
    );
    assert_eq!(tr[2]["sentenceText"], "Three more things.");
    assert_eq!(
        tr[2]["sentenceTranslations"]["vi"],
        "[vi] Three more things."
    );
}

// ---- real pipeline output into the real transcript consumers -------------------------------------

mod transcript_roundtrip {
    use super::*;
    use std::collections::HashMap;
    use warptalk_bus::{StreamHandler, StreamMessage, xadd_json};
    use warptalk_config::JwtConfig;
    use warptalk_contracts::streams;
    use warptalk_kernel::{SystemClock, WorkspaceId};
    use warptalk_transcript::application::ports::{
        AlwaysSaveRoomPolicy, InMemoryReadAccess, InMemoryRooms, StaticAiPolicy,
    };
    use warptalk_transcript::infrastructure::consumers::{
        SttResultsHandler, TranslateResultsHandler, TtsResultsHandler,
    };
    use warptalk_transcript::{MIGRATIONS, TranscriptDeps, TranscriptModule};
    use warptalk_web::JwtKeys;

    async fn deliver<T: serde::Serialize>(
        redis: &redis::aio::ConnectionManager,
        base: &str,
        value: &T,
        handler: &dyn StreamHandler,
    ) {
        let stream = format!("{base}:pipeline-test-{}", Uuid::new_v4());
        let mut conn = redis.clone();
        let id = xadd_json(&mut conn, &stream, value, 100).await.unwrap();
        let entries: Vec<(String, HashMap<String, String>)> = redis::cmd("XRANGE")
            .arg(&stream)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        let _: i64 = redis::cmd("EXPIRE")
            .arg(&stream)
            .arg(60)
            .query_async(&mut conn)
            .await
            .unwrap();
        let (eid, fields) = entries.into_iter().next().unwrap();
        assert_eq!(eid, id);
        handler
            .handle(&StreamMessage {
                stream,
                id: eid,
                fields,
            })
            .await
            .unwrap();
    }

    /// B-05, B-06, B-07, R-02, R-03: what the real stages publish is stored by the real
    /// transcript consumers with a human speaker name, offsets that fit, a probability
    /// confidence, every sentence of the translation, and flash sentences indexed as final.
    #[tokio::test]
    async fn pipeline_results_are_stored_correctly_by_the_transcript_module() {
        let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
        else {
            println!("DATABASE_URL / REDIS_URL unset; skipping");
            return;
        };
        let pool = warptalk_db::connect(&db, 5).await.unwrap();
        warptalk_db::run_migrations(&pool, &[MIGRATIONS])
            .await
            .unwrap();
        let redis = warptalk_bus::connect(&redis_url).await.unwrap();
        let rooms = Arc::new(InMemoryRooms::default());
        let module = TranscriptModule::new(TranscriptDeps {
            pool: pool.clone(),
            redis: redis.clone(),
            jwt: JwtKeys::from_config(&JwtConfig {
                secret: "test-secret-test-secret-test-secret-1234".into(),
                issuer: "test".into(),
                audience: "test".into(),
                access_ttl_seconds: 900,
                refresh_ttl_seconds: 3600,
            }),
            clock: Arc::new(SystemClock),
            rooms: rooms.clone(),
            room_policy: Arc::new(AlwaysSaveRoomPolicy),
            workspace_ai: Arc::new(StaticAiPolicy {
                retrieval: true,
                external_llm: true,
            }),
            host: rooms.clone(),
            read_access: Arc::new(InMemoryReadAccess::default()),
        });
        let ingest = module.ingest();
        let (stt_h, tr_h, tts_h) = (
            SttResultsHandler::new(ingest.clone()),
            TranslateResultsHandler::new(ingest.clone()),
            TtsResultsHandler::new(ingest),
        );

        // Real producer: the chain stage fed with a flash sentence and a 3-sentence segment.
        let chain = chain_with(true, false, fast_tts(), fast_mt());
        let (pid, user) = (ParticipantId::new(), UserId::new());
        let rig = rig_with_view(&chain, named_view(pid, user, "Ann Lee", &["vi"])).await;
        let mut flash = seg(pid, 0, 0, "A flash sentence.", true);
        flash.confidence = Some(-0.3);
        let long = seg(
            pid,
            0,
            1,
            "One fine day. Two good ideas. Three more things.",
            false,
        );
        let (flash_id, long_id) = (flash.segment_id, long.segment_id);
        for s in [flash, long, marker(pid, 0)] {
            rig.tx.send(s).await.unwrap();
        }
        drop(rig.tx);
        rig.run.await.unwrap();

        let ws = WorkspaceId::new();
        rooms.set(rig.room, ws, UserId::new());
        // stt first (a translation needs its segment), then translations, then dubbing metadata
        for r in rig.results.results() {
            deliver(&redis, streams::STT_RESULTS, &r, &stt_h).await;
        }
        for r in rig.results.translations() {
            deliver(&redis, streams::TRANSLATE_RESULTS, &r, &tr_h).await;
        }
        for r in rig.results.tts_results() {
            deliver(&redis, streams::TTS_RESULTS, &r, &tts_h).await;
        }

        type Row = (Uuid, String, Option<i32>, Option<i32>, Option<f64>, bool);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT s.id, s.speaker_name, s.start_time_ms, s.end_time_ms, s.confidence::float8, s.is_final \
             FROM transcript.transcript_segments s \
             JOIN transcript.transcripts t ON t.id = s.transcript_id \
             WHERE t.translation_room_id = $1 ORDER BY s.sequence_order",
        )
        .bind(rig.room.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for (_, name, start, end, _, is_final) in &rows {
            assert_eq!(name, "Ann Lee", "a name, not the participant id");
            let (s, e) = (start.unwrap(), end.unwrap());
            assert!((0..3_600_000).contains(&s) && e >= s, "{s} {e}");
            assert!(is_final, "flash sentences are final");
        }
        let flash_row = rows.iter().find(|r| r.0 == flash_id).unwrap();
        assert!(
            (flash_row.4.unwrap() - (-0.3f64).exp()).abs() < 1e-3,
            "{:?}",
            flash_row.4
        );

        let combined: Vec<String> = sqlx::query_scalar(
            "SELECT c.translated_text FROM transcript.segment_translation_links l \
             JOIN transcript.translation_contents c ON c.id = l.translation_content_id \
             WHERE l.segment_id = $1 AND l.is_current AND l.target_language = 'vi'",
        )
        .bind(long_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            combined,
            vec!["[vi] One fine day. [vi] Two good ideas. [vi] Three more things."]
        );

        // Both segments (the flash one included) were sent to indexing, under the human name.
        let mut conn = redis.clone();
        let reqs: Vec<(String, HashMap<String, String>)> = redis::cmd("XREVRANGE")
            .arg(streams::EMBEDDING_INDEX_REQUESTS)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(500)
            .query_async(&mut conn)
            .await
            .unwrap();
        let texts: Vec<String> = reqs
            .into_iter()
            .filter(|(_, f)| f.get("workspace_id") == Some(&ws.to_string()))
            .filter_map(|(_, f)| f.get("chunks_json").cloned())
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.iter().all(|t| t.contains("[Ann Lee] ")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("A flash sentence.")));
    }
}
