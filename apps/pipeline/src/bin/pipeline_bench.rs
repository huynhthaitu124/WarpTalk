//! Latency benchmark of the realtime pipeline host.
//!
//! ```text
//! cargo run -q -p warptalk-pipeline --bin pipeline-bench --release -- \
//!     --rooms 4 --speakers 3 --seconds 20 [--json] [--seed N] [--stt-p50-ms 774] \
//!     [--stt-p95-ms 1300] [--stt-early-ms 400] [--speech-ms 3000] [--pause-ms 2000] \
//!     [--sample-rate 48000] [--frame-ms 20] [--stt-only] [--languages 1] [--workers N] \
//!     [--baseline] [--early-endpoint-ms 250] [--no-early-mt] [--no-tts-prewarm] \
//!     [--stage-capacity 64] \
//!     [--tts-cold-prob 0.2] [--tts-concurrency 16] [--mt-p50-ms 685] [--sentence-ms 2000]
//! ```
//!
//! Default mode is the full chain (STT -> MT -> TTS -> dub) with the spec 08 provider numbers;
//! `--stt-only` runs the STT-only host. The latency flags default to the production defaults of
//! `LatencyTuning`; `--baseline` switches every optimization off (the reference run of
//! docs/perf/README.md), `--early-endpoint-ms N`, `--no-early-mt` and `--no-tts-prewarm` set
//! them one by one.

use std::process::ExitCode;
use std::str::FromStr;

use warptalk_pipeline::bench::{BenchConfig, run_bench};

const USAGE: &str = "usage: pipeline-bench [--rooms N] [--speakers N] [--seconds N] [--json] [--seed N] \
[--stt-p50-ms F] [--stt-p95-ms F] [--stt-early-ms N] [--speech-ms N] [--pause-ms N] \
[--sample-rate HZ] [--frame-ms N] [--stt-only] [--languages N] [--workers N] \
[--baseline] [--early-endpoint-ms N] [--no-early-mt] [--no-tts-prewarm] [--stage-capacity N] [--tts-cold-prob P] \
[--tts-concurrency N] [--mt-p50-ms F] [--mt-p95-ms F] [--sentence-ms N] [--speech-jitter F]";

fn value<T: FromStr>(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<T, String> {
    let raw = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
    raw.parse()
        .map_err(|_| format!("invalid value {raw:?} for {flag}"))
}

fn parse() -> Result<(BenchConfig, bool), String> {
    let mut cfg = BenchConfig {
        chain: true,
        ..BenchConfig::default()
    };
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rooms" => cfg.rooms = value(&mut args, &a)?,
            "--speakers" => cfg.speakers = value(&mut args, &a)?,
            "--seconds" => cfg.seconds = value(&mut args, &a)?,
            "--seed" => cfg.seed = value(&mut args, &a)?,
            "--stt-p50-ms" => cfg.stt_p50_ms = value(&mut args, &a)?,
            "--stt-p95-ms" => cfg.stt_p95_ms = value(&mut args, &a)?,
            "--stt-early-ms" => cfg.stt_early_ms = value(&mut args, &a)?,
            "--speech-ms" => cfg.speech_ms = value(&mut args, &a)?,
            "--pause-ms" => cfg.pause_ms = value(&mut args, &a)?,
            "--sample-rate" => cfg.sample_rate_hz = value(&mut args, &a)?,
            "--frame-ms" => cfg.frame_ms = value(&mut args, &a)?,
            "--stt-only" => cfg.chain = false,
            "--languages" => cfg.target_languages = value(&mut args, &a)?,
            "--workers" => cfg.workers = Some(value(&mut args, &a)?),
            "--early-endpoint-ms" => cfg.early_endpoint_ms = value(&mut args, &a)?,
            "--baseline" => {
                cfg.early_endpoint_ms = 0;
                cfg.early_mt = false;
                cfg.tts_prewarm = false;
            }
            "--early-mt" => cfg.early_mt = true,
            "--no-early-mt" => cfg.early_mt = false,
            "--tts-prewarm" => cfg.tts_prewarm = true,
            "--no-tts-prewarm" => cfg.tts_prewarm = false,
            "--stage-capacity" => cfg.stage_capacity = value(&mut args, &a)?,
            "--tts-cold-prob" => cfg.tts_cold_probability = value(&mut args, &a)?,
            "--tts-concurrency" => cfg.tts_concurrency = value(&mut args, &a)?,
            "--mt-p50-ms" => cfg.mt_p50_ms = value(&mut args, &a)?,
            "--mt-p95-ms" => cfg.mt_p95_ms = value(&mut args, &a)?,
            "--sentence-ms" => cfg.sentence_ms = value(&mut args, &a)?,
            "--speech-jitter" => cfg.speech_jitter = value(&mut args, &a)?,
            "--json" => json = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    if cfg.rooms == 0 || cfg.speakers == 0 || cfg.seconds == 0 || cfg.frame_ms == 0 {
        return Err("rooms, speakers, seconds and frame-ms must be > 0".to_owned());
    }
    if cfg.stt_p50_ms <= 0.0 {
        return Err("stt-p50-ms must be > 0".to_owned());
    }
    Ok((cfg, json))
}

fn main() -> ExitCode {
    let (cfg, json) = match parse() {
        Ok(v) => v,
        Err(m) => {
            eprintln!("{m}");
            return ExitCode::from(2);
        }
    };
    let mut rt = tokio::runtime::Builder::new_multi_thread();
    rt.enable_all();
    if let Some(n) = cfg.workers.filter(|n| *n > 0) {
        rt.worker_threads(n);
    }
    match rt.build() {
        Ok(rt) => rt.block_on(run(cfg, json)),
        Err(e) => {
            eprintln!("cannot start the runtime: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cfg: BenchConfig, json: bool) -> ExitCode {
    let report = run_bench(cfg).await;
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("cannot encode report: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print!("{}", report.render_table());
    }
    ExitCode::SUCCESS
}
