#![allow(clippy::unwrap_used)]
//! TranslationStage behaviour with a fake translator: ordering, speculation, gating, skips.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use warptalk_mt::testing::{FakeTranslator, segment};
use warptalk_mt::{
    ContextProvider, GlossaryTerm, InMemoryGlossary, MtConfig, MtDeps, MtEvent, MtOutputs,
    NeverSkip, NoContext, SkipPolicy, SkipReason, SpeculativeHint, StageHandle, StaticTargets,
    Translation, TranslationStage,
};

struct Rig {
    tx: mpsc::Sender<warptalk_pipeline_runtime::SttSegment>,
    hints: mpsc::Sender<SpeculativeHint>,
    out: mpsc::Receiver<Translation>,
    events: mpsc::Receiver<MtEvent>,
    fake: Arc<FakeTranslator>,
    cfg: watch::Sender<MtConfig>,
    handle: StageHandle,
}

struct Skip(Option<SkipReason>);

#[async_trait]
impl SkipPolicy for Skip {
    async fn skip_reason(&self, _r: &str, _s: &str) -> Option<SkipReason> {
        self.0
    }
}

struct Ctx(Vec<String>);

#[async_trait]
impl ContextProvider for Ctx {
    async fn meeting_context(&self, _r: &str) -> Vec<String> {
        self.0.clone()
    }
}

fn rig_with(
    targets: &[&str],
    cfg: MtConfig,
    skip: Arc<dyn SkipPolicy>,
    ctx: Arc<dyn ContextProvider>,
    glossary: Arc<InMemoryGlossary>,
) -> Rig {
    let fake = Arc::new(FakeTranslator::new());
    let (tx, rx) = mpsc::channel(64);
    let (htx, hrx) = mpsc::channel(64);
    let (otx, orx) = mpsc::channel(64);
    let (etx, erx) = mpsc::channel(64);
    let (ctx_tx, ctx_rx) = watch::channel(cfg);
    let handle = TranslationStage::spawn(
        MtDeps {
            translator: fake.clone(),
            glossary,
            targets: Arc::new(StaticTargets(targets.iter().map(|s| (*s).into()).collect())),
            skip,
            context: ctx,
        },
        ctx_rx,
        rx,
        Some(hrx),
        MtOutputs {
            translations: otx,
            events: Some(etx),
        },
    );
    Rig {
        tx,
        hints: htx,
        out: orx,
        events: erx,
        fake,
        cfg: ctx_tx,
        handle,
    }
}

fn rig(targets: &[&str]) -> Rig {
    rig_with(
        targets,
        MtConfig::default(),
        Arc::new(NeverSkip),
        Arc::new(NoContext),
        Arc::new(InMemoryGlossary::new()),
    )
}

async fn recv(r: &mut Rig) -> Translation {
    tokio::time::timeout(Duration::from_secs(3), r.out.recv())
        .await
        .expect("timed out waiting for a translation")
        .expect("output closed")
}

async fn finish(r: Rig) -> Vec<Translation> {
    let Rig {
        tx,
        mut out,
        handle,
        ..
    } = r;
    drop(tx);
    handle.join().await;
    let mut v = Vec::new();
    while let Ok(t) = out.try_recv() {
        v.push(t);
    }
    v
}

#[tokio::test]
async fn later_sentence_never_overtakes_earlier_with_out_of_order_completion() {
    let mut r = rig(&["vi"]);
    r.fake
        .delay_for("Slow first sentence.", Duration::from_millis(250));
    r.fake.delay_for("Quick second.", Duration::from_millis(5));
    r.fake.delay_for("Quick third.", Duration::from_millis(5));
    for (i, t) in ["Slow first sentence.", "Quick second.", "Quick third."]
        .iter()
        .enumerate()
    {
        r.tx.send(segment("room", "alice", i as u64, t, "en"))
            .await
            .unwrap();
    }
    let t0 = Instant::now();
    let got = [recv(&mut r).await, recv(&mut r).await, recv(&mut r).await];
    // The fast ones were ready long before, yet they come out after the slow one.
    assert!(t0.elapsed() >= Duration::from_millis(240));
    assert_eq!(
        got.iter().map(|t| t.seq).collect::<Vec<_>>(),
        [0, 1, 2],
        "per-speaker order"
    );
    assert_eq!(got[0].text, "[vi] Slow first sentence.");
    // Translation itself ran concurrently: total well under the sum of sequential delays.
    assert!(t0.elapsed() < Duration::from_millis(600));
}

#[tokio::test]
async fn other_speakers_are_not_blocked_by_a_slow_one() {
    let mut r = rig(&["vi"]);
    r.fake.delay_for("Slow alice.", Duration::from_millis(300));
    r.tx.send(segment("room", "alice", 0, "Slow alice.", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "bob", 0, "Fast bob.", "en"))
        .await
        .unwrap();
    let first = recv(&mut r).await;
    assert_eq!(first.speaker, "bob");
    assert_eq!(recv(&mut r).await.speaker, "alice");
}

#[tokio::test]
async fn bounded_order_wait_then_goes_out_and_nothing_is_dropped() {
    let cfg = MtConfig {
        publish_order_wait_ms: 100,
        ..MtConfig::default()
    };
    let mut r = rig_with(
        &["vi"],
        cfg,
        Arc::new(NeverSkip),
        Arc::new(NoContext),
        Arc::new(InMemoryGlossary::new()),
    );
    r.fake.delay_for("Stalled one.", Duration::from_millis(700));
    r.tx.send(segment("room", "alice", 0, "Stalled one.", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "alice", 1, "Next one.", "en"))
        .await
        .unwrap();
    let t0 = Instant::now();
    let first = recv(&mut r).await;
    assert_eq!(first.seq, 1, "B goes out after the bounded wait");
    assert!(t0.elapsed() >= Duration::from_millis(95) && t0.elapsed() < Duration::from_millis(600));
    let second = recv(&mut r).await;
    assert_eq!(second.seq, 0, "A is still published when it lands");
    let mut saw = false;
    while let Ok(e) = r.events.try_recv() {
        saw |= matches!(e, MtEvent::OrderWaitExceeded { .. });
    }
    assert!(saw);
}

#[tokio::test]
async fn fans_out_per_target_language_concurrently() {
    let r = rig(&["vi", "ja-JP", "vi-VN", "en"]);
    r.fake.delay_for("Hello team.", Duration::from_millis(120));
    r.tx.send(segment("room", "alice", 0, "Hello team.", "en"))
        .await
        .unwrap();
    let t0 = Instant::now();
    let got = finish(r).await;
    // vi and vi-VN collapse to one language; en is the speaker's own (passthrough).
    let mut langs: Vec<_> = got
        .iter()
        .map(|t| (t.target_lang.clone(), t.passthrough))
        .collect();
    langs.sort();
    assert_eq!(
        langs,
        [
            ("en".to_string(), true),
            ("ja-JP".to_string(), false),
            ("vi".to_string(), false)
        ]
    );
    assert!(
        t0.elapsed() < Duration::from_millis(250),
        "languages ran concurrently"
    );
    let en = got.iter().find(|t| t.target_lang == "en").unwrap();
    assert_eq!(en.text, "Hello team.");
    assert!(en.model.starts_with("passthrough"));
}

#[tokio::test]
async fn first_sentence_is_published_before_the_batch_finishes() {
    let mut r = rig(&["vi"]);
    r.fake.delay_for("One.", Duration::from_millis(10));
    r.fake.delay_for("Two.", Duration::from_millis(300));
    r.fake.delay_for("Three.", Duration::from_millis(300));
    r.tx.send(segment("room", "alice", 0, "One. Two. Three.", "en"))
        .await
        .unwrap();
    let t0 = Instant::now();
    let first = recv(&mut r).await;
    assert_eq!(first.chunk_idx, 0);
    assert!(
        t0.elapsed() < Duration::from_millis(200),
        "first sentence not held for the batch"
    );
    let (second, third) = (recv(&mut r).await, recv(&mut r).await);
    assert_eq!((second.chunk_idx, third.chunk_idx), (1, 2));
    assert!(third.is_final_chunk && !first.is_final_chunk);
    // Sentence 0 alone; sentences 1..N in ONE batch call.
    assert_eq!(r.fake.single_calls.load(Ordering::SeqCst), 1);
    assert_eq!(r.fake.batch_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second.model, "fake-batch");
}

#[tokio::test]
async fn speculative_hit_avoids_second_call_and_is_not_published_early() {
    let mut r = rig(&["vi"]);
    r.fake
        .delay_for("Deploy to staging.", Duration::from_millis(80));
    r.hints
        .send(SpeculativeHint {
            room: "room".into(),
            speaker: "alice".into(),
            text: "deploy to staging".into(), // normalised key: case and trailing dot ignored
            lang: "en".into(),
        })
        .await
        .unwrap();
    // Speculation alone publishes nothing.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(r.out.try_recv().is_err());
    assert_eq!(r.handle.speculative_entries(), 1);
    r.fake
        .delay_for("deploy to staging", Duration::from_millis(80));
    r.tx.send(segment("room", "alice", 0, "Deploy to staging.", "en"))
        .await
        .unwrap();
    let t = recv(&mut r).await;
    assert_eq!(t.text, "[vi] deploy to staging");
    assert_eq!(
        r.fake.single_calls.load(Ordering::SeqCst),
        1,
        "one model call in total"
    );
    assert_eq!(r.handle.speculative_entries(), 0, "a hit is consumed");
}

#[tokio::test]
async fn speculative_miss_translates_on_demand_and_ttl_expiry_is_a_miss() {
    let cfg = MtConfig {
        speculative: warptalk_mt::SpeculativeConfig {
            ttl_ms: 50,
            ..Default::default()
        },
        ..MtConfig::default()
    };
    let mut r = rig_with(
        &["vi"],
        cfg,
        Arc::new(NeverSkip),
        Arc::new(NoContext),
        Arc::new(InMemoryGlossary::new()),
    );
    r.hints
        .send(SpeculativeHint {
            room: "room".into(),
            speaker: "alice".into(),
            text: "Old news.".into(),
            lang: "en".into(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    r.tx.send(segment("room", "alice", 0, "Old news.", "en"))
        .await
        .unwrap();
    let _ = recv(&mut r).await;
    assert_eq!(
        r.fake.single_calls.load(Ordering::SeqCst),
        2,
        "expired entry was not used"
    );
}

#[tokio::test]
async fn glossary_only_matching_terms_and_repair_gating() {
    let g = Arc::new(InMemoryGlossary::new());
    g.set_global(vec![
        GlossaryTerm::new("staging", "moi truong staging"),
        GlossaryTerm::new("Kubernetes", ""),
        GlossaryTerm::new("billing", ""),
    ]);
    let mut r = rig_with(
        &["vi"],
        MtConfig::default(),
        Arc::new(NeverSkip),
        Arc::new(NoContext),
        g,
    );
    r.tx.send(segment("room", "alice", 0, "Push it to staging.", "en"))
        .await
        .unwrap();
    let t = recv(&mut r).await;
    assert_eq!(t.text, "[vi] Push it to staging.");
    let req = r.fake.requests.lock().unwrap()[0].clone();
    let names: Vec<_> = req
        .glossary
        .iter()
        .map(|s| s.term.source.as_str())
        .collect();
    assert_eq!(names, ["staging"], "only terms that occur in the source");
    assert!(
        !req.repair,
        "exact hit alone, no context, high confidence: no repair clause"
    );
    assert!(req.context.is_empty());

    // The next segment of the same room carries context, which is evidence: repair opens.
    r.tx.send(segment("room", "alice", 1, "Okay now ship.", "en"))
        .await
        .unwrap();
    let _ = recv(&mut r).await;
    let reqs = r.fake.requests.lock().unwrap().clone();
    let second = reqs.iter().find(|q| q.text == "Okay now ship.").unwrap();
    assert!(second.repair);
    assert_eq!(second.context, ["Push it to staging."]);
    assert!(second.glossary.is_empty());
}

#[tokio::test]
async fn low_confidence_opens_repair_without_context() {
    let mut r = rig(&["en"]);
    let mut seg = segment("room", "alice", 0, "Chay kubernetis di.", "vi");
    seg.confidence = Some(-0.9);
    r.tx.send(seg).await.unwrap();
    let _ = recv(&mut r).await;
    let mut confident = segment("room2", "alice", 0, "Chay di.", "vi");
    confident.confidence = Some(-0.05);
    r.tx.send(confident).await.unwrap();
    let _ = recv(&mut r).await;
    let reqs = r.fake.requests.lock().unwrap().clone();
    assert!(
        reqs.iter()
            .find(|q| q.text.contains("kubernetis"))
            .unwrap()
            .repair
    );
    assert!(!reqs.iter().find(|q| q.text == "Chay di.").unwrap().repair);
}

#[tokio::test]
async fn static_meeting_context_is_included_and_bounded() {
    let mut r = rig_with(
        &["vi"],
        MtConfig::default(),
        Arc::new(NeverSkip),
        Arc::new(Ctx(vec![
            "Topic: Q3 launch".into(),
            "About the new billing".into(),
        ])),
        Arc::new(InMemoryGlossary::new()),
    );
    r.tx.send(segment("room", "alice", 0, "Ship it.", "en"))
        .await
        .unwrap();
    let _ = recv(&mut r).await;
    let req = r.fake.requests.lock().unwrap()[0].clone();
    assert_eq!(req.context, ["Topic: Q3 launch", "About the new billing"]);
    assert!(req.repair);
}

#[tokio::test]
async fn skip_rules_spend_nothing_and_do_not_block_successors() {
    // Control marker and empty text: no call at all.
    let mut r = rig(&["vi"]);
    r.tx.send(segment("room", "system", 0, "__MEETING_END__", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "alice", 0, "   ", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "alice", 1, "Real one.", "en"))
        .await
        .unwrap();
    let t = recv(&mut r).await;
    assert_eq!(t.seq, 1);
    assert_eq!(r.fake.total_calls(), 1);
    let mut reasons = Vec::new();
    while let Ok(MtEvent::Skipped { reason, .. }) = r.events.try_recv() {
        reasons.push(reason);
    }
    assert_eq!(reasons, [SkipReason::ControlMarker, SkipReason::Empty]);

    // Room-state skips (credits / not started / paused) happen before the model call.
    let mut r = rig_with(
        &["vi"],
        MtConfig::default(),
        Arc::new(Skip(Some(SkipReason::CreditsExhausted))),
        Arc::new(NoContext),
        Arc::new(InMemoryGlossary::new()),
    );
    r.tx.send(segment("room", "alice", 0, "No credits.", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "alice", 1, "Still none.", "en"))
        .await
        .unwrap();
    let got = finish_with_events(&mut r).await;
    assert!(got.is_empty());
    assert_eq!(r.fake.total_calls(), 0);
    assert!(matches!(
        r.events.try_recv().unwrap(),
        MtEvent::Skipped {
            reason: SkipReason::CreditsExhausted,
            ..
        }
    ));
}

async fn finish_with_events(r: &mut Rig) -> Vec<Translation> {
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut v = Vec::new();
    while let Ok(t) = r.out.try_recv() {
        v.push(t);
    }
    v
}

#[tokio::test]
async fn failure_emits_event_and_successors_still_flow() {
    let mut r = rig(&["vi"]);
    r.fake.fail_on("Broken.");
    r.tx.send(segment("room", "alice", 0, "Broken.", "en"))
        .await
        .unwrap();
    r.tx.send(segment("room", "alice", 1, "Works.", "en"))
        .await
        .unwrap();
    let t = recv(&mut r).await;
    assert_eq!(t.seq, 1);
    let mut failed = false;
    while let Ok(e) = r.events.try_recv() {
        failed |= matches!(e, MtEvent::Failed { .. });
    }
    assert!(
        failed,
        "the failure is reported; the source transcript path is unaffected"
    );
}

#[tokio::test]
async fn stamps_and_contract_conversion() {
    let mut r = rig(&["vi"]);
    r.tx.send(segment("room", "alice", 0, "Hi there.", "en"))
        .await
        .unwrap();
    let t = recv(&mut r).await;
    assert!(t.stamps.mt_first_token.unwrap() >= t.stamps.mt_start);
    assert!(t.stamps.mt_done.unwrap() >= t.stamps.mt_first_token.unwrap());
    let c = t.to_contract();
    assert_eq!(c.target_lang, "vi");
    assert_eq!(c.translated_text, "[vi] Hi there.");
    assert!(c.segment_id.ends_with("-vi-c0"));
    assert_eq!(c.translator_model.as_deref(), Some("fake"));
}

#[tokio::test]
async fn hot_reload_changes_behaviour_for_the_next_segment() {
    let g = Arc::new(InMemoryGlossary::new());
    g.set_global(vec![GlossaryTerm::new("staging", "")]);
    let mut r = rig_with(
        &["vi"],
        MtConfig::default(),
        Arc::new(NeverSkip),
        Arc::new(NoContext),
        g,
    );
    r.tx.send(segment("room", "alice", 0, "Go to staging.", "en"))
        .await
        .unwrap();
    let _ = recv(&mut r).await;
    r.cfg
        .send(MtConfig {
            glossary_limit: 0,
            ..MtConfig::default()
        })
        .unwrap();
    r.tx.send(segment("room", "alice", 1, "Back to staging.", "en"))
        .await
        .unwrap();
    let _ = recv(&mut r).await;
    let reqs = r.fake.requests.lock().unwrap().clone();
    assert_eq!(reqs[0].glossary.len(), 1);
    assert!(reqs[1].glossary.is_empty(), "config is re-read per segment");
}
