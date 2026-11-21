#![allow(clippy::unwrap_used)]
//! TtsStage behaviour with a fake vendor: streaming, cache, gate, delivery, voices, degradation.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc, watch};
use uuid::Uuid;
use warptalk_mt::testing::segment;
use warptalk_mt::{MtStamps, Translation};
use warptalk_tts::testing::{FailKind, FakeCatalog, FakeCloner, FakeSynth, StaticConsent};
use warptalk_tts::{
    CloneLadder, DubChunk, InMemoryTtsCache, NoProfiles, RouteTier, TtsConfig, TtsDeps, TtsEvent,
    TtsOutputs, TtsStage, TtsStageHandle, VoiceMode, VoiceResolver,
};

struct Rig {
    tx: mpsc::Sender<Translation>,
    chunks: mpsc::Receiver<DubChunk>,
    events: mpsc::Receiver<TtsEvent>,
    synth: Arc<FakeSynth>,
    cloner: Arc<FakeCloner>,
    ladder: Arc<CloneLadder>,
    cache: Arc<InMemoryTtsCache>,
    cfg: watch::Sender<TtsConfig>,
    handle: TtsStageHandle,
}

fn fast_cfg() -> TtsConfig {
    let mut c = TtsConfig::default();
    c.retry.backoff_ms = 10;
    c
}

fn rig(cfg: TtsConfig, consent: bool) -> Rig {
    let synth = FakeSynth::new(24_000);
    let cloner = Arc::new(FakeCloner::default());
    let cache = Arc::new(InMemoryTtsCache::default());
    let (cfg_tx, cfg_rx) = watch::channel(cfg);
    let ladder = CloneLadder::new(cfg_rx.clone(), cloner.clone());
    let voices = Arc::new(VoiceResolver {
        cfg: cfg_rx.clone(),
        consent: Arc::new(StaticConsent(consent)),
        ladder: ladder.clone(),
        profiles: Arc::new(NoProfiles),
        catalog: Arc::new(FakeCatalog(vec!["stock-a".into(), "stock-b".into()])),
    });
    let (tx, rx) = mpsc::channel(64);
    let (ctx, crx) = mpsc::channel(256);
    let (etx, erx) = mpsc::channel(64);
    let handle = TtsStage::spawn(
        TtsDeps {
            synth: Arc::new(synth.clone()),
            cache: cache.clone(),
            voices,
        },
        cfg_rx,
        rx,
        TtsOutputs {
            chunks: ctx,
            events: Some(etx),
        },
    );
    Rig {
        tx,
        chunks: crx,
        events: erx,
        synth,
        cloner,
        ladder,
        cache,
        cfg: cfg_tx,
        handle,
    }
}

fn tr(speaker: &str, lang: &str, idx: u32, text: &str) -> Translation {
    tr_seg(Uuid::now_v7(), speaker, lang, idx, text)
}

fn tr_seg(seg: Uuid, speaker: &str, lang: &str, idx: u32, text: &str) -> Translation {
    let s = segment("room", speaker, 0, text, "en");
    Translation {
        segment_id: seg,
        chunk_idx: idx,
        is_final_chunk: true,
        room: "room".into(),
        speaker: speaker.into(),
        seq: 0,
        utterance_seq: 0,
        source_lang: "en".into(),
        target_lang: lang.into(),
        source_text: text.into(),
        text: text.into(),
        model: "fake".into(),
        passthrough: false,
        is_early: false,
        confidence: None,
        source_ms: None,
        stamps: MtStamps {
            stt: s.stamps,
            mt_start: Instant::now(),
            mt_first_token: Some(Instant::now()),
            mt_done: Some(Instant::now()),
        },
    }
}

async fn next_chunk(r: &mut Rig) -> DubChunk {
    tokio::time::timeout(Duration::from_secs(3), r.chunks.recv())
        .await
        .expect("timed out waiting for a chunk")
        .expect("closed")
}

/// Chunks of one sentence up to and including `is_last`.
async fn sentence(r: &mut Rig) -> Vec<DubChunk> {
    let mut v = Vec::new();
    loop {
        let c = next_chunk(r).await;
        let last = c.is_last;
        v.push(c);
        if last {
            return v;
        }
    }
}

fn drain_events(r: &mut Rig) -> Vec<TtsEvent> {
    let mut v = Vec::new();
    while let Ok(e) = r.events.try_recv() {
        v.push(e);
    }
    v
}

async fn wait_events(r: &mut Rig, min: usize) -> Vec<TtsEvent> {
    let mut v = Vec::new();
    for _ in 0..300 {
        v.extend(drain_events(r));
        if v.len() >= min {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    v
}

fn speechlike(secs: f32) -> Vec<f32> {
    let n = (secs * 16_000.0) as usize;
    (0..n)
        .map(|i| 0.3 * (0.5 + 0.5 * (i as f32 / 1600.0).sin().abs()) * (i as f32 * 0.07).sin())
        .collect()
}

#[tokio::test]
async fn first_chunk_is_forwarded_before_the_last_arrives() {
    let mut r = rig(fast_cfg(), false);
    let hold = Arc::new(Notify::new());
    r.synth.with_script(|s| s.hold_last = Some(hold.clone()));
    r.tx.send(tr("alice", "vi", 0, "Xin chao moi nguoi"))
        .await
        .unwrap();
    // The vendor is still holding its last chunk; the first two are already out.
    let a = next_chunk(&mut r).await;
    let b = next_chunk(&mut r).await;
    assert_eq!((a.seq, b.seq), (0, 1));
    assert!(!a.is_last && !b.is_last);
    assert_eq!(a.sample_rate, 24_000);
    assert!(a.stamps.tts_first_audio.is_some());
    assert!(r.chunks.try_recv().is_err(), "nothing else yet");
    hold.notify_one();
    let c = next_chunk(&mut r).await;
    let end = next_chunk(&mut r).await;
    assert_eq!(c.pcm.len(), 480);
    assert!(end.is_last && end.pcm.is_empty());
    assert_eq!(end.seq, 3);
    assert_eq!(a.voice.mode, VoiceMode::Standard);
    let ev = wait_events(&mut r, 1).await;
    let done = ev
        .iter()
        .find(|e| matches!(e, TtsEvent::SentenceDone { .. }))
        .unwrap();
    let TtsEvent::SentenceDone { duration_ms, .. } = done else {
        unreachable!()
    };
    assert_eq!(*duration_ms, 60); // 1440 samples at 24 kHz
    assert!(done.to_contract().unwrap().segment_id.ends_with("-vi-c0"));
}

#[tokio::test]
async fn cache_hit_skips_the_provider_and_replays_in_chunks() {
    let mut r = rig(fast_cfg(), false);
    r.tx.send(tr("alice", "vi", 0, "Cung mot cau"))
        .await
        .unwrap();
    let first = sentence(&mut r).await;
    for _ in 0..200 {
        if !r.cache.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(r.cache.len(), 1);
    r.tx.send(tr("alice", "vi", 0, "cung  MOT cau"))
        .await
        .unwrap(); // normalised same key
    let second = sentence(&mut r).await;
    assert_eq!(r.synth.speak_count(), 1, "provider called once");
    assert!(second.iter().all(|c| c.cache_hit));
    assert!(second.last().unwrap().is_last);
    let total = |v: &[DubChunk]| v.iter().map(|c| c.pcm.len()).sum::<usize>();
    assert_eq!(total(&first), total(&second));
    // A different delivery (catch-up) is a different render: key includes it.
    let mut late = tr("alice", "vi", 0, "Cung mot cau");
    late.stamps.stt.speech_end = Instant::now().checked_sub(Duration::from_secs(5));
    r.tx.send(late).await.unwrap();
    let third = sentence(&mut r).await;
    assert!(!third[0].cache_hit);
    assert_eq!(r.synth.speak_count(), 2);
}

#[tokio::test]
async fn slot_gate_limits_concurrent_generations() {
    let cfg = TtsConfig {
        max_concurrency: 2,
        ..fast_cfg()
    };
    let mut r = rig(cfg, false);
    r.synth
        .with_script(|s| s.chunk_delay = Duration::from_millis(40));
    for i in 0..6 {
        r.tx.send(tr(&format!("spk{i}"), "vi", 0, "Mot cau dai du de do"))
            .await
            .unwrap();
    }
    let ev = wait_events(&mut r, 6).await;
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, TtsEvent::SentenceDone { .. }))
            .count(),
        6
    );
    let max = r.synth.max_active.load(Ordering::SeqCst);
    assert!(max <= 2, "vendor saw {max} concurrent generations");
    assert!(max >= 2, "the gate should allow its full width");
    let waited = ev
        .iter()
        .any(|e| matches!(e, TtsEvent::SentenceDone { slot_wait_ms, .. } if *slot_wait_ms > 20));
    assert!(waited, "later sentences report their slot wait");
    assert_eq!(r.handle.slots_in_use(), 0, "every slot released");
}

#[tokio::test]
async fn hot_reload_resizes_the_gate() {
    let cfg = TtsConfig {
        max_concurrency: 1,
        ..fast_cfg()
    };
    let mut r = rig(cfg, false);
    r.synth
        .with_script(|s| s.chunk_delay = Duration::from_millis(60));
    r.cfg
        .send(TtsConfig {
            max_concurrency: 4,
            ..fast_cfg()
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    for i in 0..4 {
        r.tx.send(tr(&format!("s{i}"), "vi", 0, "Cau cho kiem tra"))
            .await
            .unwrap();
    }
    wait_events(&mut r, 4).await;
    assert_eq!(r.synth.max_active.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn catch_up_speed_follows_lag() {
    let mut r = rig(fast_cfg(), false);
    for (i, lag_ms) in [0u64, 2750, 4000].into_iter().enumerate() {
        let mut t = tr("alice", "vi", i as u32, &format!("Cau so {i} day"));
        if lag_ms > 0 {
            t.stamps.stt.speech_end = Instant::now().checked_sub(Duration::from_millis(lag_ms));
        }
        r.tx.send(t).await.unwrap();
        sentence(&mut r).await;
    }
    let recs = r.synth.records();
    assert_eq!(recs[0].cfg.speed, None, "on time: say nothing");
    let mid = recs[1].cfg.speed.unwrap();
    assert!((mid - 1.125).abs() < 0.01, "{mid}");
    let full = recs[2].cfg.speed.unwrap();
    assert!((full - 1.25).abs() < 0.01, "{full}");
}

#[tokio::test]
async fn isochrony_centres_speed_after_a_fit_is_established() {
    let mut r = rig(fast_cfg(), false);
    // The fake dub is 60 ms; with source_ms 480 the ratio is 0.125, implausible and skipped,
    // so use a long fake dub: 3 chunks of 4800 samples = 600 ms against a 400 ms source.
    r.synth.with_script(|s| s.chunk_samples = 4800);
    for i in 0..3 {
        let mut t = tr("alice", "vi", i, &format!("Dong so {i} nhe"));
        t.source_ms = Some(400);
        r.tx.send(t).await.unwrap();
        sentence(&mut r).await;
    }
    let recs = r.synth.records();
    assert_eq!(recs[0].cfg.speed, None);
    assert_eq!(recs[1].cfg.speed, None, "one sample is not a fit yet");
    let s = recs[2].cfg.speed.unwrap();
    assert!(
        (s - 1.25).abs() < 0.01,
        "ratio 1.5 clamped to 1.25, got {s}"
    );
}

#[tokio::test]
async fn clone_creation_failure_falls_back_to_standard_and_emits_degraded() {
    let mut r = rig(fast_cfg(), true);
    r.cloner.fail.store(true, Ordering::SeqCst);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    for _ in 0..200 {
        if r.ladder.failed("room", "bob") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    r.tx.send(tr("bob", "vi", 0, "Cau noi cua bob"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Standard);
    let ev = wait_events(&mut r, 2).await;
    assert!(ev.iter().any(|e| matches!(
        e,
        TtsEvent::RouteDegraded { from: RouteTier::Clone, to: RouteTier::Standard, reason, .. } if reason == "clone_failed"
    )));
    // A second line does not repeat the event.
    r.tx.send(tr("bob", "vi", 1, "Cau tiep theo day"))
        .await
        .unwrap();
    sentence(&mut r).await;
    let ev = wait_events(&mut r, 1).await;
    assert!(
        !ev.iter()
            .any(|e| matches!(e, TtsEvent::RouteDegraded { .. }))
    );
}

#[tokio::test]
async fn clone_synthesis_failure_retries_with_standard_voice() {
    let mut r = rig(fast_cfg(), true);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    for _ in 0..200 {
        if r.ladder.current("room", "bob").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let clone = r.ladder.current("room", "bob").unwrap().voice_id;
    r.synth.with_script(|s| {
        s.fail_voices.insert(clone.clone(), FailKind::Permanent);
    });
    r.tx.send(tr("bob", "vi", 0, "Cau noi cua bob"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Standard);
    assert!(c[0].voice.voice_id.starts_with("stock-"));
    let ev = wait_events(&mut r, 2).await;
    assert!(ev.iter().any(|e| matches!(
        e,
        TtsEvent::RouteDegraded {
            from: RouteTier::Clone,
            to: RouteTier::Standard,
            ..
        }
    )));
    let recs = r.synth.records();
    assert_eq!(recs[0].voice_id, clone);
    assert!(recs[1].voice_id.starts_with("stock-"));
}

#[tokio::test]
async fn clone_is_used_when_ready_and_first_dub_waits_bounded() {
    let mut r = rig(fast_cfg(), true);
    *r.cloner.delay.lock().unwrap() = Duration::from_millis(200);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    // The clone is still being created: the first dub waits for it (bounded by first_wait_ms).
    let t0 = Instant::now();
    r.tx.send(tr("bob", "vi", 0, "Cau noi cua bob"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Cloned);
    assert!(t0.elapsed() >= Duration::from_millis(150));
    // Text shorter than min_clone_chars uses the stock voice.
    r.tx.send(tr("bob", "vi", 1, "Vang")).await.unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Standard);
}

#[tokio::test]
async fn first_wait_gives_up_after_the_bound() {
    let cfg = {
        let mut c = fast_cfg();
        c.clone.first_wait_ms = 100;
        c
    };
    let mut r = rig(cfg, true);
    *r.cloner.delay.lock().unwrap() = Duration::from_millis(800);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    let t0 = Instant::now();
    r.tx.send(tr("bob", "vi", 0, "Cau noi cua bob"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Standard);
    assert!(t0.elapsed() < Duration::from_millis(600));
}

#[tokio::test]
async fn no_consent_never_clones_and_is_not_a_degradation() {
    let mut r = rig(fast_cfg(), false);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    r.tx.send(tr("bob", "vi", 0, "Cau noi cua bob"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].voice.mode, VoiceMode::Standard);
    let ev = wait_events(&mut r, 1).await;
    assert!(
        !ev.iter()
            .any(|e| matches!(e, TtsEvent::RouteDegraded { .. }))
    );
}

#[tokio::test]
async fn ladder_upgrades_only_with_margin_and_revoke_deletes() {
    let r = rig(fast_cfg(), true);
    r.ladder.push_audio("room", "bob", "en", &speechlike(3.5));
    for _ in 0..200 {
        if r.ladder.current("room", "bob").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let first = r.ladder.current("room", "bob").unwrap();
    // 8 s rung: an equal-quality clone does not beat the 0.15 margin, so the old one stays.
    r.ladder.push_audio("room", "bob", "en", &speechlike(5.0));
    for _ in 0..200 {
        if r.cloner.calls.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(r.ladder.current("room", "bob").unwrap(), first);
    assert_eq!(
        r.cloner.deleted.lock().unwrap().len(),
        1,
        "the rejected rung is deleted"
    );
    r.ladder.revoke("room", "bob").await;
    assert!(r.ladder.current("room", "bob").is_none());
    assert!(r.cloner.deleted.lock().unwrap().contains(&first.voice_id));
}

#[tokio::test]
async fn fan_out_synthesizes_once_for_two_listeners_of_the_same_language() {
    let mut r = rig(fast_cfg(), false);
    let seg = Uuid::now_v7();
    // Two listeners of Vietnamese: the same translation requested twice.
    r.tx.send(tr_seg(seg, "alice", "vi", 0, "Mot ban dich cho hai nguoi"))
        .await
        .unwrap();
    r.tx.send(tr_seg(seg, "alice", "vi", 0, "Mot ban dich cho hai nguoi"))
        .await
        .unwrap();
    sentence(&mut r).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        r.synth.speak_count(),
        1,
        "one synthesis serves both listeners"
    );
    assert!(r.chunks.try_recv().is_err(), "and one stream of chunks");
    // A second language is a different key: synthesized separately.
    r.tx.send(tr_seg(seg, "alice", "ja", 0, "Mot ban dich cho hai nguoi"))
        .await
        .unwrap();
    sentence(&mut r).await;
    assert_eq!(r.synth.speak_count(), 2);
}

#[tokio::test]
async fn transient_error_is_retried_once_in_order() {
    let mut r = rig(fast_cfg(), false);
    r.synth
        .with_script(|s| s.fail_next.push_back(FailKind::Transient));
    r.tx.send(tr("alice", "vi", 0, "Cau dau tien"))
        .await
        .unwrap();
    r.tx.send(tr("alice", "vi", 1, "Cau thu hai"))
        .await
        .unwrap();
    let a = sentence(&mut r).await;
    let b = sentence(&mut r).await;
    assert_eq!(
        (a[0].chunk_idx, b[0].chunk_idx),
        (0, 1),
        "retry never reorders"
    );
    assert_eq!(r.synth.speak_count(), 3);
}

#[tokio::test]
async fn permanent_error_dead_letters_and_degrades_to_text_only() {
    let mut r = rig(fast_cfg(), false);
    r.synth
        .with_script(|s| s.fail_next.push_back(FailKind::Permanent));
    r.tx.send(tr("alice", "vi", 0, "Cau bi tu choi"))
        .await
        .unwrap();
    let ev = wait_events(&mut r, 2).await;
    assert_eq!(r.synth.speak_count(), 1, "no retry for a refusal");
    assert!(
        ev.iter()
            .any(|e| matches!(e, TtsEvent::DeadLetter { reason, .. } if reason == "not_retryable"))
    );
    assert!(ev.iter().any(|e| matches!(
        e,
        TtsEvent::RouteDegraded {
            from: RouteTier::Standard,
            to: RouteTier::TextOnly,
            ..
        }
    )));
    // The next line still flows and recovers the route.
    r.tx.send(tr("alice", "vi", 1, "Cau tiep theo"))
        .await
        .unwrap();
    sentence(&mut r).await;
}

#[tokio::test]
async fn partially_spoken_line_is_never_replayed() {
    let mut r = rig(fast_cfg(), false);
    r.synth
        .with_script(|s| s.fail_next.push_back(FailKind::PartialThenTransient));
    r.tx.send(tr("alice", "vi", 0, "Cau noi dang do"))
        .await
        .unwrap();
    let c = sentence(&mut r).await; // one real chunk, then the closing marker
    assert_eq!(c.len(), 2);
    assert!(c[1].is_last && c[1].pcm.is_empty());
    let ev = wait_events(&mut r, 2).await;
    assert_eq!(r.synth.speak_count(), 1);
    assert!(
        ev.iter().any(
            |e| matches!(e, TtsEvent::DeadLetter { reason, .. } if reason == "partially_spoken")
        )
    );
}

#[tokio::test]
async fn context_is_continued_when_next_sentence_is_queued_and_closed_otherwise() {
    let mut r = rig(fast_cfg(), false);
    r.synth
        .with_script(|s| s.chunk_delay = Duration::from_millis(40));
    r.tx.send(tr("alice", "vi", 0, "Cau mot cua luot noi"))
        .await
        .unwrap();
    r.tx.send(tr("alice", "vi", 1, "Cau hai cua luot noi"))
        .await
        .unwrap();
    sentence(&mut r).await;
    sentence(&mut r).await;
    let ev = wait_events(&mut r, 2).await;
    assert_eq!(
        r.synth.opened.load(Ordering::SeqCst),
        1,
        "one context for both sentences"
    );
    let continued: Vec<bool> = ev
        .iter()
        .filter_map(|e| match e {
            TtsEvent::SentenceDone { continued, .. } => Some(*continued),
            _ => None,
        })
        .collect();
    assert_eq!(continued, [false, true]);
    // Nothing queued after the last sentence: the context is ended at once (frees the slot).
    for _ in 0..100 {
        if r.synth.closed_gracefully.load(Ordering::SeqCst) == 1 && r.handle.slots_in_use() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(r.synth.closed_gracefully.load(Ordering::SeqCst), 1);
    assert_eq!(r.handle.slots_in_use(), 0);
}

#[tokio::test]
async fn passthrough_and_empty_text_are_not_dubbed() {
    let mut r = rig(fast_cfg(), false);
    let mut p = tr("alice", "en", 0, "Same language caption");
    p.passthrough = true;
    r.tx.send(p).await.unwrap();
    r.tx.send(tr("alice", "vi", 0, "   ")).await.unwrap();
    r.tx.send(tr("alice", "vi", 1, "Mot cau that su"))
        .await
        .unwrap();
    let c = sentence(&mut r).await;
    assert_eq!(c[0].chunk_idx, 1);
    assert_eq!(r.synth.speak_count(), 1);
}
