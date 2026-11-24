#![allow(clippy::unwrap_used)]
//! End-to-end run of the fake provider through the real engine and summary service.
//! Needs Redis and Postgres (REDIS_URL / DATABASE_URL, localhost defaults); skips when absent.

use std::path::Path;

use warptalk_ai_eval::dataset;
use warptalk_ai_eval::harness::{DEFAULT_DATABASE_URL, DEFAULT_REDIS_URL, RunOptions, Suite, run};
use warptalk_ai_eval::providers::ProviderSpec;
use warptalk_ai_eval::report::{Report, compare};

fn opts(suite: Suite) -> RunOptions {
    RunOptions {
        suite,
        provider: ProviderSpec::Fake,
        model: "fake-1".into(),
        judge: None,
        meetings: dataset::load_dir(&dataset::default_dir(), &[]).unwrap(),
        tolerance: 1,
        suggestion_overrides: None,
        summary_overrides: None,
        price: None,
        redis_url: std::env::var("REDIS_URL").unwrap_or(DEFAULT_REDIS_URL.into()),
        database_url: std::env::var("DATABASE_URL").unwrap_or(DEFAULT_DATABASE_URL.into()),
    }
}

#[tokio::test]
async fn fake_suite_runs_end_to_end_and_matches_baseline() {
    let report = match run(&opts(Suite::All)).await {
        Ok(r) => r,
        Err(e) if e.to_string().contains("cannot reach") => {
            println!("skipping: {e}");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    let s = report.suggestions.as_ref().unwrap();
    assert_eq!(s.meetings.len(), 6);
    assert!(
        s.metrics.published > 20 && s.metrics.recall > 0.5,
        "{:?}",
        s.metrics
    );
    assert!(
        s.metrics.hallucination_hits >= 1,
        "the fake falls into a trap on purpose"
    );
    let m = &report.summary.as_ref().unwrap().metrics;
    assert_eq!(m.counts.failed, 0);
    assert!(m.decision_recall > 0.9);
    assert!(m.counts.trap_hits >= 1);

    let baseline =
        Report::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines/fake.json")).unwrap();
    let regressions = compare(&report, &baseline, 0.05);
    assert!(regressions.is_empty(), "{regressions:?}");
    // The comparison catches a regression.
    let mut worse = report.clone();
    if let Some(s) = worse.suggestions.as_mut() {
        s.metrics.recall -= 0.2;
    }
    assert!(!compare(&worse, &report, 0.05).is_empty());
}
