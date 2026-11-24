#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use warptalk_copilot_core::{
    Channel, CopilotConfig, CopilotDeps, CopilotEngine, CopilotEvent, EchoAction, FakeStt,
    MemorySink, PushStatus, StatusKind,
};
use warptalk_ingress::EndpointerConfig;

fn big_queue_cfg() -> CopilotConfig {
    let mut c = CopilotConfig::default();
    c.me.queue_capacity_chunks = 100_000;
    c.others.queue_capacity_chunks = 100_000;
    c.engine.reconnect_backoff_ms = 0;
    c
}

fn make(
    cfg: CopilotConfig,
) -> (
    CopilotEngine,
    tokio::sync::mpsc::Receiver<CopilotEvent>,
    FakeStt,
    Arc<MemorySink>,
) {
    let stt = FakeStt::new(Duration::ZERO);
    let sink = MemorySink::new();
    let (e, rx) = CopilotEngine::new(
        cfg,
        CopilotDeps {
            stt: Arc::new(stt.clone()),
            sink: sink.clone(),
            session_id: "s1".into(),
        },
    )
    .unwrap();
    (e, rx, stt, sink)
}

/// Pushes both tracks in lockstep, 20 ms chunks, as fast as possible.
fn push_lockstep(e: &CopilotEngine, me: (&[f32], u32), others: (&[f32], u32)) {
    let n = (me.0.len() as u64 * 1000 / u64::from(me.1))
        .max(others.0.len() as u64 * 1000 / u64::from(others.1));
    let mut t = 0;
    while t < n {
        for (ch, (s, r)) in [(Channel::Me, me), (Channel::Others, others)] {
            let per = (u64::from(r) * 20 / 1000) as usize;
            let from = (t / 20) as usize * per;
            if from < s.len() {
                e.push_audio(ch, &s[from..(from + per).min(s.len())], r);
            }
        }
        t += 20;
    }
}

fn near(a: u64, b: u64, tol: u64) -> bool {
    a.abs_diff(b) <= tol
}

#[tokio::test]
async fn two_channels_yield_ordered_segments_with_offsets() {
    let (e, mut rx, _stt, sink) = make(big_queue_cfg());
    // me @48 kHz: speech 500..2000 and 6000..7500; others @16 kHz: speech 3000..4500.
    let me = track(48_000, 0.3, &[-500, 1500, -4000, 1500, -1500]);
    let others = track(16_000, 0.3, &[-3000, 1500, -4500]);
    push_lockstep(&e, (&me, 48_000), (&others, 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(200), |_| false).await;
    let segs = segments(&ev);
    assert_eq!(segs.len(), 3, "{segs:#?}");

    let mut by_time = segs.clone();
    by_time.sort_by_key(|s| s.start_ms);
    let order: Vec<_> = by_time
        .iter()
        .map(|s| (s.channel, s.text.as_str()))
        .collect();
    assert_eq!(
        order,
        vec![
            (Channel::Me, "utterance 0 on me"),
            (Channel::Others, "utterance 0 on others"),
            (Channel::Me, "utterance 1 on me"),
        ]
    );
    let expect = [(500, 2000), (3000, 4500), (6000, 7500)];
    for (s, (st, en)) in by_time.iter().zip(expect) {
        assert!(near(s.start_ms, st, 100), "start {} vs {st}", s.start_ms);
        assert!(near(s.end_ms, en, 100), "end {} vs {en}", s.end_ms);
        assert!(s.is_final && !s.likely_echo);
        assert!(s.start_ms <= s.end_ms);
        assert_eq!(s.client_segment_id.len(), 36);
    }
    // Per-channel emission order is preserved.
    let me_segs: Vec<_> = segs.iter().filter(|s| s.channel == Channel::Me).collect();
    assert!(me_segs[0].start_ms < me_segs[1].start_ms);
    // Sink got the same segments, unique ids.
    let sunk = sink.segments();
    assert_eq!(sunk.len(), 3);
    let mut ids: Vec<_> = sunk.iter().map(|s| s.client_segment_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

#[tokio::test]
async fn events_include_status_and_levels() {
    let (e, mut rx, _s, _k) = make(big_queue_cfg());
    let me = track(16_000, 0.3, &[1000, -2000]);
    push_lockstep(&e, (&me, 16_000), (&[], 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(200), |_| false).await;
    let has = |k: StatusKind| {
        ev.iter()
            .any(|e| matches!(e, CopilotEvent::Status { kind, .. } if *kind == k))
    };
    assert!(has(StatusKind::Started));
    assert!(has(StatusKind::SpeechStart));
    assert!(has(StatusKind::SpeechEnd));
    assert!(has(StatusKind::SttConnected));
    assert!(has(StatusKind::Stopped));
    assert!(
        ev.iter()
            .any(|e| matches!(e, CopilotEvent::Levels { channel: Channel::Me, rms } if *rms > 0.1))
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e, CopilotEvent::Partial { text, .. } if text.is_empty()))
    );
}

#[tokio::test]
async fn echo_guard_marks_bleed_but_not_real_speech() {
    let (e, mut rx, _s, sink) = make(big_queue_cfg());
    // 0.5..2.0 s: others loud, mic hears it faintly (-20 dB): echo.
    // 5.0..6.5 s: the user speaks alone, loudly: not echo.
    // 9.0..10.5 s: both talk at similar level: genuine double talk, not echo.
    let me = track(16_000, 0.03, &[-500, 1500, -3000, 0, -0]); // faint part
    let mut me_full = me.clone();
    me_full.extend(track(16_000, 0.3, &[1500, -2500, 1500, -1500]));
    let mut others = track(16_000, 0.5, &[-500, 1500, -7000]);
    others.extend(track(16_000, 0.3, &[1500, -1500]));
    // me timeline: 500 sil,1500 faint,3000 sil | 1500 loud (at 5000),2500 sil,1500 loud (at 9000)
    push_lockstep(&e, (&me_full, 16_000), (&others, 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(200), |_| false).await;
    let mut me_segs: Vec<_> = segments(&ev)
        .into_iter()
        .filter(|s| s.channel == Channel::Me)
        .collect();
    me_segs.sort_by_key(|s| s.start_ms);
    assert_eq!(me_segs.len(), 3, "{me_segs:#?}");
    assert!(me_segs[0].likely_echo, "bleed must be marked");
    assert!(!me_segs[1].likely_echo, "solo speech must not be marked");
    assert!(
        !me_segs[2].likely_echo,
        "double talk at equal level must not be marked"
    );
    assert!(
        segments(&ev)
            .iter()
            .filter(|s| s.channel == Channel::Others)
            .all(|s| !s.likely_echo)
    );
    assert_eq!(e.stats().me.echo_marked, 1);
    assert_eq!(sink.segments().len(), 5, "Mark keeps everything");
}

#[tokio::test]
async fn echo_drop_action_and_threshold_config() {
    let mut cfg = big_queue_cfg();
    cfg.echo.action = EchoAction::Drop;
    let (e, mut rx, _s, sink) = make(cfg);
    let me = track(16_000, 0.03, &[-500, 1500, -2000]);
    let others = track(16_000, 0.5, &[-500, 1500, -2000]);
    push_lockstep(&e, (&me, 16_000), (&others, 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(100), |_| false).await;
    let segs = segments(&ev);
    assert!(
        segs.iter()
            .any(|s| s.channel == Channel::Me && s.likely_echo)
    );
    // Dropped from the sink, still emitted as an event.
    assert!(sink.segments().iter().all(|s| s.channel == Channel::Others));

    // A 40 dB threshold makes the same audio count as real speech.
    let mut cfg = big_queue_cfg();
    cfg.echo.dominance_db = 40.0;
    let (e, mut rx, _s, _k) = make(cfg);
    push_lockstep(&e, (&me, 16_000), (&others, 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(100), |_| false).await;
    assert!(segments(&ev).iter().all(|s| !s.likely_echo));
}

#[tokio::test]
async fn push_never_blocks_and_drops_oldest_when_consumer_stalled() {
    // Current-thread runtime and no await between pushes: the worker cannot run at all, so
    // if push blocked on the consumer this test would hang.
    let mut cfg = CopilotConfig::default();
    cfg.me.queue_capacity_chunks = 10;
    let (e, _rx, _s, _k) = make(cfg);
    let chunk = tone(48_000, 20, 0.3);
    let t0 = Instant::now();
    let mut dropped = 0;
    for _ in 0..20_000 {
        if e.push_audio(Channel::Me, &chunk, 48_000) == PushStatus::DroppedOldest {
            dropped += 1;
        }
    }
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "push is not O(1)/non-blocking"
    );
    assert_eq!(dropped, 19_990);
    let st = e.stats();
    assert_eq!(st.me.chunks_dropped, 19_990);
    assert_eq!(st.me.chunks_pushed, 20_000);
    assert_eq!(e.push_audio(Channel::Me, &[], 16_000), PushStatus::Ignored);
    e.stop().await;
    assert_eq!(
        e.push_audio(Channel::Me, &chunk, 48_000),
        PushStatus::Closed
    );
}

#[tokio::test]
async fn session_rotates_at_utterance_boundaries_without_losing_segments() {
    let mut cfg = big_queue_cfg();
    cfg.engine.session_rotate_after_ms = 1;
    let (e, mut rx, stt, sink) = make(cfg);
    let me = track(16_000, 0.3, &[-300, 1200, -2500, 1200, -2500, 1200, -2500]);
    push_lockstep(&e, (&me, 16_000), (&[], 16_000));
    e.stop().await;
    let _ = collect_until(&mut rx, Duration::from_millis(100), |_| false).await;
    assert_eq!(sink.segments().len(), 3);
    assert!(
        stt.sessions_opened() >= 4,
        "opened {}",
        stt.sessions_opened()
    );
    assert_eq!(stt.commits(), 3);
}

#[tokio::test]
async fn failed_stt_open_is_retried_and_reported() {
    let (e, mut rx, stt, sink) = {
        let stt = FakeStt::new(Duration::ZERO);
        stt.fail_next_opens(1);
        let sink = MemorySink::new();
        let (e, rx) = CopilotEngine::new(
            big_queue_cfg(),
            CopilotDeps {
                stt: Arc::new(stt.clone()),
                sink: sink.clone(),
                session_id: "s".into(),
            },
        )
        .unwrap();
        (e, rx, stt, sink)
    };
    let me = track(16_000, 0.3, &[-300, 1200, -2500]);
    push_lockstep(&e, (&me, 16_000), (&[], 16_000));
    e.stop().await;
    let ev = collect_until(&mut rx, Duration::from_millis(100), |_| false).await;
    assert!(
        ev.iter()
            .any(|e| matches!(e, CopilotEvent::Error { code, .. } if code == "stt_open"))
    );
    assert_eq!(sink.segments().len(), 1);
    // One session per channel; the failed attempt is not counted.
    assert_eq!(stt.sessions_opened(), 2);
}

#[tokio::test]
async fn endpointer_hot_reload_applies_when_idle() {
    let (e, mut rx, _s, sink) = make(big_queue_cfg());
    // 400 ms burst is accepted by default (min speech 288 ms).
    let burst = track(16_000, 0.3, &[400, -1500]);
    for c in burst.chunks(320) {
        e.push_audio(Channel::Me, c, 16_000);
    }
    let _ = collect_until(&mut rx, Duration::from_secs(5), |ev| {
        !segments(ev).is_empty()
    })
    .await;
    assert_eq!(sink.segments().len(), 1);

    // Raise the minimum speech length to 1 s: the same burst is now dropped.
    let ep = warptalk_ingress::EndpointerConfig {
        vad_min_speech_ms: 1000,
        ..Default::default()
    };
    e.update_endpointer(Channel::Me, ep).unwrap();
    let bad = warptalk_ingress::EndpointerConfig {
        vad_threshold: 5.0,
        ..Default::default()
    };
    assert!(e.update_endpointer(Channel::Me, bad).is_err());
    for c in burst.chunks(320) {
        e.push_audio(Channel::Me, c, 16_000);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    e.stop().await;
    assert_eq!(
        sink.segments().len(),
        1,
        "short burst should be dropped after reload"
    );
}

#[test]
fn config_defaults_json_and_validation() {
    let c = CopilotConfig::from_json("{}").unwrap();
    assert!(c.echo.enabled);
    let c = CopilotConfig::from_json(
        r#"{"echo":{"dominance_db":6.5,"action":"drop"},"me":{"endpointer":{"vad_threshold":0.5}},"stt":{"backend":"fake"}}"#,
    )
    .unwrap();
    assert_eq!(c.echo.dominance_db, 6.5);
    assert_eq!(c.echo.action, EchoAction::Drop);
    assert_eq!(c.me.endpointer.vad_threshold, 0.5);
    assert_eq!(c.others.endpointer.vad_threshold, 0.35);
    assert!(CopilotConfig::from_json(r#"{"sink":{"batch_max":0}}"#).is_err());
    assert!(CopilotConfig::from_json(r#"{"me":{"endpointer":{"chunk_duration_ms":1}}}"#).is_err());
    assert!(CopilotConfig::from_json("not json").is_err());
}

/// Bench (run with `--ignored --nocapture`): per-frame cost and end-of-speech -> segment latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn bench_frame_cost_and_latency() {
    // 1. Throughput: 5 minutes of two channels at 48 kHz in 10 ms chunks.
    let (e, _rx, _s, _k) = make(big_queue_cfg());
    let secs = 300u64;
    let me = track(48_000, 0.3, &[1500, -1500].repeat((secs / 3) as usize));
    let chunk = 480;
    let t0 = Instant::now();
    let mut n = 0u64;
    for c in me.chunks(chunk) {
        e.push_audio(Channel::Me, c, 48_000);
        e.push_audio(Channel::Others, c, 48_000);
        n += 2;
    }
    let push_elapsed = t0.elapsed();
    e.stop().await;
    let total = t0.elapsed();
    println!(
        "push: {n} chunks, {:.2} us/chunk (caller side); end-to-end {:.2} s for {} s x2 audio -> {:.1}x real time; frames(32ms)/s processed: {:.0}",
        push_elapsed.as_secs_f64() * 1e6 / n as f64,
        total.as_secs_f64(),
        secs,
        (2 * secs) as f64 / total.as_secs_f64(),
        (2 * secs * 1000 / 32) as f64 / total.as_secs_f64()
    );
    println!(
        "per 32 ms frame (both channels, incl. resample+VAD+endpointer): {:.2} us",
        total.as_secs_f64() * 1e6 / ((secs * 1000 / 32) as f64 * 2.0)
    );

    // 2. Latency at real-time pace: last speech chunk pushed -> segment event received.
    let (e, mut rx, _s, _k) = make(big_queue_cfg());
    let (seg_tx, seg_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut seg_tx = Some(seg_tx);
        while let Some(ev) = rx.recv().await {
            if matches!(ev, CopilotEvent::Segment { .. })
                && let Some(t) = seg_tx.take()
            {
                let _ = t.send(Instant::now());
            }
        }
    });
    let audio = track(16_000, 0.3, &[-500, 1500, -3000]);
    let t0 = Instant::now();
    let mut last_speech_pushed = t0;
    for (i, c) in audio.chunks(320).enumerate() {
        let at = i as u64 * 20;
        tokio::time::sleep_until(tokio::time::Instant::from_std(
            t0 + Duration::from_millis(at),
        ))
        .await;
        e.push_audio(Channel::Me, c, 16_000);
        if (500..2000).contains(&at) {
            last_speech_pushed = Instant::now();
        }
    }
    let got = tokio::time::timeout(Duration::from_secs(3), seg_rx)
        .await
        .unwrap()
        .unwrap();
    let lat = got.duration_since(last_speech_pushed);
    println!(
        "end-of-speech -> segment: {} ms total; nominal hangover {} ms (1.5 s turn) => pipeline overhead ~{} ms",
        lat.as_millis(),
        EndpointerConfig::default().vad_silence_hangover_ms,
        lat.as_millis() as i64 - i64::from(EndpointerConfig::default().vad_silence_hangover_ms)
    );
    e.stop().await;
}
