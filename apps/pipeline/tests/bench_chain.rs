#![allow(clippy::unwrap_used)]
//! The chain benchmark (own process: it installs the global metrics recorder).

use warptalk_pipeline::bench::{BenchConfig, run_bench};

#[tokio::test]
async fn chain_bench_reports_every_stage() {
    let r = run_bench(BenchConfig {
        rooms: 1,
        speakers: 2,
        seconds: 9,
        chain: true,
        speech_ms: 2000,
        pause_ms: 2500,
        sentence_ms: 4000,
        ..BenchConfig::default()
    })
    .await;
    for stage in [
        "endpoint_decision",
        "mt_first_token",
        "mt_done",
        "tts_first_audio",
        "e2e_first_caption",
        "e2e_first_translated_caption",
        "e2e_first_dub_audio",
    ] {
        assert!(r.stages[stage].count > 0, "no samples for {stage}: {r:?}");
    }
    let chain = r.chain.as_ref().unwrap();
    assert!(chain.dub_chunks_published > 0);
    assert!(chain.translated_captions > 0);
    assert_eq!(chain.translate_results, chain.tts_results);
    assert_eq!(r.frames_dropped, 0);
    assert_eq!(r.media_events_dropped, 0);
    // Production defaults: MT starts on the early sentence, so the dub beats the turn-end path
    // (hangover + STT completion + MT + TTS is about 2.3 s).
    let dub = &r.stages["e2e_first_dub_audio"];
    assert!(dub.p50 < 2300.0, "first dub p50 {} ms", dub.p50);
    serde_json::to_string(&r).unwrap();
    println!("{}", r.render_table());
}
