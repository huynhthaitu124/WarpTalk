#![allow(clippy::unwrap_used)]
//! Early endpoint through the runtime: STT sentence terminator -> `hint_sentence_complete` ->
//! shorter hangover. Frames are paced in real time because the STT double runs on wall time.

use std::sync::Arc;
use std::time::Duration;

use warptalk_ingress::EndpointerConfig;
use warptalk_pipeline_runtime::testing::{
    JitterStt, JitterSttConfig, energy_vad_factory, silence_frame, tone_frame,
};
use warptalk_pipeline_runtime::{PipelineConfig, RoomDeps, RoomPipeline, SttSegment};

/// Speaks `speech_ms`, then stays silent `silence_ms`, one frame per 32 ms of wall time.
async fn paced_turn(h: &warptalk_pipeline_runtime::SpeakerHandle, speech_ms: u32, silence_ms: u32) {
    let mut tick = tokio::time::interval(Duration::from_millis(32));
    for i in 0..speech_ms / 32 {
        tick.tick().await;
        h.push(tone_frame(0.3, i as usize));
    }
    for _ in 0..silence_ms / 32 {
        tick.tick().await;
        h.push(silence_frame());
    }
}

async fn run(hint_ms: u32, stt_cfg: JitterSttConfig) -> (Vec<u32>, Vec<SttSegment>) {
    let stt = Arc::new(JitterStt::new(stt_cfg));
    let cfg = PipelineConfig {
        endpointer: EndpointerConfig {
            vad_sentence_hint_hangover_ms: hint_ms,
            ..EndpointerConfig::default()
        },
        emit_utterance_end: true,
        ..PipelineConfig::default()
    };
    let (rp, mut rx) = RoomPipeline::spawn(
        RoomDeps {
            room: "r".into(),
            stt: stt.clone(),
            vad_factory: energy_vad_factory(),
            external_shutdown: None,
        },
        cfg,
    )
    .unwrap();
    let h = rp.add_speaker("a", Some("en".into())).await.unwrap();
    paced_turn(&h, 1600, 1600).await;
    let mut segs = Vec::new();
    while let Ok(Some(s)) = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
        let end = s.utterance_end;
        segs.push(s);
        if end {
            break;
        }
    }
    rp.shutdown().await;
    (stt.log.hangovers(), segs)
}

fn fast_tail() -> JitterSttConfig {
    JitterSttConfig {
        sentence_ms: 10_000, // no mid-turn sentence: only the trailing one
        tail_lag_p50_ms: 300.0,
        tail_lag_p95_ms: 300.0,
        commit_to_done_p50_ms: 100.0,
        commit_to_done_p95_ms: 100.0,
        ..JitterSttConfig::default()
    }
}

#[tokio::test]
async fn sentence_hint_cuts_the_hangover_when_enabled() {
    let (hang, segs) = run(250, fast_tail()).await;
    assert_eq!(hang.len(), 1);
    // Terminator at ~300 ms of silence >= 250: the turn closes there instead of at 576 ms.
    assert!(hang[0] < 420, "hangover {} ms, expected ~300", hang[0]);
    let texts: Vec<_> = segs
        .iter()
        .map(|s| (s.text.as_str(), s.is_early, s.utterance_end))
        .collect();
    assert_eq!(
        texts[0],
        ("The closing part of turn 0 said here.", true, false)
    );
    assert!(
        segs.last().unwrap().utterance_end,
        "end marker closes the turn"
    );
    assert!(segs.last().unwrap().stamps.speech_end.is_some());
}

#[tokio::test]
async fn without_the_flag_the_ladder_hangover_applies() {
    let (hang, _) = run(0, fast_tail()).await;
    assert_eq!(hang.len(), 1);
    assert!(hang[0] >= 576, "hangover {} ms", hang[0]);
}

#[tokio::test]
async fn late_terminator_after_the_ladder_changes_nothing() {
    // The STT only completes the sentence after the commit: nothing to hint on.
    let cfg = JitterSttConfig {
        tail_lag_p50_ms: 900.0,
        tail_lag_p95_ms: 900.0,
        ..fast_tail()
    };
    let (hang, _) = run(250, cfg).await;
    assert!(hang[0] >= 576, "hangover {} ms", hang[0]);
}
