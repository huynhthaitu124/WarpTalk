#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use warptalk_pipeline_runtime::testing::{
    FakeStt, FakeSttConfig, energy_vad_factory, utterance_frames,
};
use warptalk_pipeline_runtime::{
    PipelineConfig, PipelineError, RoomDeps, RoomPipeline, SpeakerHandle, SttMode, SttSegment,
};

fn room(stt: Arc<FakeStt>, cfg: PipelineConfig) -> (RoomPipeline, mpsc::Receiver<SttSegment>) {
    RoomPipeline::spawn(
        RoomDeps {
            room: "room-1".into(),
            stt,
            vad_factory: energy_vad_factory(),
            external_shutdown: None,
        },
        cfg,
    )
    .unwrap()
}

fn feed(h: &SpeakerHandle, speech_ms: u32, silence_ms: u32) {
    for f in utterance_frames(speech_ms, silence_ms) {
        h.push(f);
    }
}

async fn collect(rx: &mut mpsc::Receiver<SttSegment>, n: usize) -> Vec<SttSegment> {
    let mut v = Vec::new();
    while v.len() < n {
        let s = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for segments")
            .expect("channel closed early");
        v.push(s);
    }
    v
}

#[tokio::test]
async fn per_speaker_order_under_three_concurrent_speakers() {
    let mut delays = HashMap::new();
    delays.insert("a".to_string(), Duration::from_millis(60));
    delays.insert("b".to_string(), Duration::from_millis(15));
    delays.insert("c".to_string(), Duration::ZERO);
    let stt = Arc::new(FakeStt::new(FakeSttConfig {
        delay_by_speaker: delays,
        ..FakeSttConfig::default()
    }));
    let (rp, mut rx) = room(stt.clone(), PipelineConfig::default());

    let mut tasks = Vec::new();
    for id in ["a", "b", "c"] {
        let h = rp.add_speaker(id, Some("en".into())).await.unwrap();
        tasks.push(tokio::spawn(async move {
            for _ in 0..4 {
                feed(&h, 1000, 1000);
                tokio::task::yield_now().await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    // 3 speakers * 4 utterances * (early + final)
    let segs = collect(&mut rx, 24).await;
    for id in ["a", "b", "c"] {
        let mine: Vec<_> = segs.iter().filter(|s| s.speaker == id).collect();
        assert_eq!(mine.len(), 8, "speaker {id}");
        for (i, s) in mine.iter().enumerate() {
            assert_eq!(s.seq, i as u64, "seq contiguous for {id}");
            assert_eq!(s.room, "room-1");
            let expected = format!("{id}-{}-{}", i / 2, if i % 2 == 0 { "e" } else { "f" });
            assert_eq!(s.text, expected);
            assert_eq!(s.utterance_seq, (i / 2) as u64);
            assert_eq!(s.is_early, i % 2 == 0);
        }
    }
    // Commits of one speaker never overlap utterance numbering.
    let recs = stt.commit_records();
    for id in ["a", "b", "c"] {
        let seqs: Vec<_> = recs
            .iter()
            .filter(|r| r.speaker == id)
            .map(|r| r.utterance_seq)
            .collect();
        assert_eq!(seqs, vec![0, 1, 2, 3]);
    }
    rp.shutdown().await;
}

#[tokio::test]
async fn overflow_drops_oldest_without_blocking_the_source() {
    // STT feed stalls 40 ms per frame: the worker cannot keep up with a 2000-frame burst.
    let stt = Arc::new(FakeStt::new(FakeSttConfig {
        feed_delay: Duration::from_millis(40),
        ..FakeSttConfig::default()
    }));
    let cfg = PipelineConfig {
        frame_queue_capacity: 8,
        ..PipelineConfig::default()
    };
    let (rp, _rx) = room(stt, cfg);
    let h = rp.add_speaker("a", None).await.unwrap();
    let frames = utterance_frames(60_000, 0);
    let t0 = Instant::now();
    for f in frames {
        h.push(f);
    }
    let took = t0.elapsed();
    assert!(took < Duration::from_millis(500), "push blocked: {took:?}");
    assert!(h.dropped_frames() > 1000, "dropped {}", h.dropped_frames());
    assert!(h.queued_frames() <= 8);
    // Do not wait for the slow drain.
    drop(rp);
}

#[tokio::test]
async fn hot_reload_changes_hangover_on_next_utterance() {
    let stt = Arc::new(FakeStt::new(FakeSttConfig::default()));
    let (rp, mut rx) = room(stt.clone(), PipelineConfig::default());
    let h = rp.add_speaker("a", None).await.unwrap();

    feed(&h, 2000, 800);
    collect(&mut rx, 2).await;

    let mut cfg = PipelineConfig::default();
    cfg.endpointer.vad_silence_hangover_ms = 320;
    rp.update_config(cfg).await.unwrap();

    feed(&h, 2000, 800);
    collect(&mut rx, 2).await;

    let recs = stt.commit_records();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].hangover_ms, 576, "default ladder middle rung");
    assert_eq!(recs[1].hangover_ms, 320, "reloaded hangover");
    // Pre-speech padding survived the endpointer rebuild (384 ms before speech).
    assert!(recs[1].audio_samples >= recs[0].audio_samples - 16_000 / 10);

    // Invalid reload is rejected and leaves the running config alone.
    let mut bad = PipelineConfig::default();
    bad.endpointer.chunk_duration_ms = 5;
    assert!(matches!(
        rp.update_config(bad).await,
        Err(PipelineError::InvalidConfig(_))
    ));
    rp.shutdown().await;
}

#[tokio::test]
async fn stamps_are_monotonic_in_closed_chunk_mode() {
    let stt = Arc::new(FakeStt::new(FakeSttConfig {
        default_delay: Duration::from_millis(30),
        early_delay: Duration::from_millis(5),
        ..FakeSttConfig::default()
    }));
    let cfg = PipelineConfig {
        flash_mode: false,
        stt_mode: SttMode::ClosedChunk,
        ..PipelineConfig::default()
    };
    let (rp, mut rx) = room(stt.clone(), cfg);
    let h = rp.add_speaker("a", None).await.unwrap();
    feed(&h, 1500, 1000);
    let segs = collect(&mut rx, 2).await;

    // No live frames in closed-chunk mode; the whole audio went with the commit.
    let rec = &stt.commit_records()[0];
    assert_eq!(rec.streamed_samples, 0);
    assert!(rec.audio_samples > 0);
    assert_eq!(
        stt.fed_samples.load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    let fin = &segs[1];
    assert!(!fin.is_early);
    let s = fin.stamps;
    let (end, commit, first, last) = (
        s.speech_end.unwrap(),
        s.stt_commit.unwrap(),
        s.stt_first_text.unwrap(),
        s.stt_final.unwrap(),
    );
    assert!(s.audio_arrived <= end && end <= commit && commit <= first && first <= last);
    assert!(last.duration_since(commit) >= Duration::from_millis(30));
    // The early segment saw the same turn.
    assert_eq!(segs[0].stamps.stt_commit, segs[1].stamps.stt_commit);
    rp.shutdown().await;
}

#[tokio::test]
async fn flash_mode_streams_live_frames_before_commit() {
    let stt = Arc::new(FakeStt::new(FakeSttConfig::default()));
    let (rp, mut rx) = room(stt.clone(), PipelineConfig::default());
    let h = rp.add_speaker("a", None).await.unwrap();
    feed(&h, 1500, 1000);
    collect(&mut rx, 2).await;
    let rec = &stt.commit_records()[0];
    assert!(rec.streamed_samples > 0, "live frames were fed");
    assert_eq!(
        stt.fed_samples.load(std::sync::atomic::Ordering::Relaxed),
        rec.streamed_samples
    );
    rp.shutdown().await;
}

#[tokio::test]
async fn shutdown_drains_open_utterance_and_inflight_transcripts() {
    let stt = Arc::new(FakeStt::new(FakeSttConfig {
        default_delay: Duration::from_millis(150),
        ..FakeSttConfig::default()
    }));
    let (rp, mut rx) = room(stt, PipelineConfig::default());
    let h = rp.add_speaker("a", None).await.unwrap();
    // Speech only: the utterance is still open when shutdown arrives.
    feed(&h, 1500, 0);
    rp.shutdown().await;

    let mut got = Vec::new();
    while let Some(s) = rx.recv().await {
        got.push(s);
    }
    assert_eq!(
        got.len(),
        2,
        "early + final delivered before the channel closed"
    );
    assert_eq!(got[1].text, "a-0-f");
    assert!(matches!(
        rp.add_speaker("b", None).await,
        Err(PipelineError::RoomClosed)
    ));
}

#[tokio::test]
async fn remove_speaker_drains_and_unknown_speaker_errors() {
    let stt = Arc::new(FakeStt::new(FakeSttConfig::default()));
    let (rp, mut rx) = room(stt, PipelineConfig::default());
    let h = rp.add_speaker("a", None).await.unwrap();
    assert!(matches!(
        rp.add_speaker("a", None).await,
        Err(PipelineError::SpeakerExists(_))
    ));
    feed(&h, 1500, 0);
    rp.remove_speaker("a").await.unwrap();
    let segs = collect(&mut rx, 2).await;
    assert_eq!(segs[0].speaker, "a");
    assert!(matches!(
        rp.remove_speaker("a").await,
        Err(PipelineError::SpeakerNotFound(_))
    ));
    rp.shutdown().await;
}
