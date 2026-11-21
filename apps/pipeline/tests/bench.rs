#![allow(clippy::unwrap_used)]
//! The benchmark harness itself: 2 rooms x 2 speakers for 5 s (own process: it installs the
//! global metrics recorder).

use warptalk_pipeline::bench::{BenchConfig, run_bench};

#[tokio::test]
async fn bench_runs_two_rooms_two_speakers() {
    let started = std::time::Instant::now();
    let r = run_bench(BenchConfig {
        rooms: 2,
        speakers: 2,
        seconds: 5,
        ..BenchConfig::default()
    })
    .await;
    assert!(started.elapsed().as_secs() < 10);
    assert_eq!(r.config.rooms, 2);
    for stage in ["endpoint_decision", "stt_final", "e2e_final_caption"] {
        assert!(r.stages[stage].count > 0, "no samples for {stage}: {r:?}");
    }
    assert!(r.stt_results_published > 0);
    assert!(r.captions_published >= r.stt_results_published);
    assert_eq!(r.frames_dropped, 0);
    let p = &r.stages["endpoint_decision"];
    assert!(p.p50 > 200.0 && p.p50 < 1200.0, "hangover p50 {}", p.p50);
    serde_json::to_string(&r).unwrap();
    println!("{}", r.render_table());
}
