//! Command line: `run` and `report`. Hand-rolled parsing (no extra dependencies).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::bench;
use crate::cost::Price;
use crate::dataset;
use crate::harness::{DEFAULT_DATABASE_URL, DEFAULT_REDIS_URL, JudgeSetup, RunOptions, Suite, run};
use crate::providers;
use crate::report::{Report, compare, misses, to_markdown};

pub const USAGE: &str = "\
warptalk-ai-eval: offline evaluation of the AI core

USAGE
  warptalk-ai-eval run [OPTIONS]
  warptalk-ai-eval bench-latency [OPTIONS]
  warptalk-ai-eval report --in report.json [--compare baseline.json] [--threshold 0.03]

run OPTIONS
  --suite suggestions|summary|all     default all
  --provider fake|fake-naive|openai|anthropic|gemini   default fake (keys from OPENAI_API_KEY,
                                      ANTHROPIC_API_KEY, GEMINI_API_KEY). fake-naive obeys prompt
                                      injections: it must score a high injection_success_rate
  --model NAME                        required for real providers
  --judge-provider P --judge-model M  optional LLM-as-judge (faithfulness 1-5)
  --out report.json                   write the JSON report
  --compare baseline.json             exit 1 when a quality metric regressed
  --threshold 0.03                    allowed absolute drop (default 0.03)
  --dataset-dir DIR                   default: the bundled datasets
  --meeting ID                        only this meeting (repeatable)
  --tolerance N                       lines after at_line that still count (default 1)
  --suggestion-config file.json       overrides merged into the suggestion config
  --summary-config file.json          overrides for the summary config
  --price-in X --price-out Y          USD per 1M tokens for the cost estimate
  --redis-url URL / --database-url URL   default REDIS_URL / DATABASE_URL, then localhost

bench-latency OPTIONS  (segment end -> suggestion published, via the worker's engine path)
  --provider / --model                as for run (fake uses simulated model latency)
  --meeting ID                        meeting(s) to replay (default: the first dataset meeting)
  --sessions 1,10,50                  concurrent sessions, one level each (default 1,10,50)
  --speed X                           replay speed (default 1; model latency is not scaled, so
                                      X > 1 raises the load)
  --stagger-ms N                      spread session starts over N ms (default 3000)
  --max-pipelines N                   global model pipelines, like the worker (default 8)
  --decide-ms MEAN[:SD] --generate-ms MEAN[:SD]   fake latency (default 300:100 / 700:200)
  --seed N                            fake latency seed (default 1)
  --suggestion-config file.json       engine config overrides
  --out bench.json --md-out bench.md  write the JSON / markdown report
  --target-answer-p95-ms N            exit 1 when the answer p95 of any level exceeds N

Exit codes: 0 ok, 1 regression against the baseline, 2 error.
";

#[derive(Default)]
struct Flags(HashMap<String, Vec<String>>);

impl Flags {
    fn parse(args: &[String]) -> Result<Self> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let Some(name) = a.strip_prefix("--") else {
                bail!("unexpected argument `{a}`");
            };
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n.to_owned(), v.to_owned()),
                None => {
                    let v = it
                        .next()
                        .with_context(|| format!("--{name} needs a value"))?;
                    (name.to_owned(), v.clone())
                }
            };
            m.entry(name).or_default().push(value);
        }
        Ok(Self(m))
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(|v| v.last()).map(String::as_str)
    }

    fn all(&self, k: &str) -> Vec<String> {
        self.0.get(k).cloned().unwrap_or_default()
    }

    fn number<T: std::str::FromStr>(&self, k: &str) -> Result<Option<T>> {
        self.get(k)
            .map(|s| {
                s.parse::<T>()
                    .map_err(|_| anyhow::anyhow!("--{k}: bad number `{s}`"))
            })
            .transpose()
    }

    fn json_file(&self, k: &str) -> Result<Option<Value>> {
        self.get(k)
            .map(|p| {
                let s = std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?;
                serde_json::from_str(&s).with_context(|| format!("parsing {p}"))
            })
            .transpose()
    }
}

pub async fn main(args: Vec<String>) -> ExitCode {
    match dispatch(&args).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn dispatch(args: &[String]) -> Result<ExitCode> {
    match args.first().map(String::as_str) {
        Some("run") => cmd_run(&Flags::parse(&args[1..])?).await,
        Some("bench-latency") => cmd_bench(&Flags::parse(&args[1..])?).await,
        Some("report") => cmd_report(&Flags::parse(&args[1..])?),
        Some("help" | "--help" | "-h") | None => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn check_baseline(current: &Report, flags: &Flags) -> Result<ExitCode> {
    let Some(path) = flags.get("compare") else {
        return Ok(ExitCode::SUCCESS);
    };
    let threshold = flags.number::<f64>("threshold")?.unwrap_or(0.03);
    let baseline = Report::load(Path::new(path))?;
    let regressions = compare(current, &baseline, threshold);
    if regressions.is_empty() {
        eprintln!("no regression against {path} (threshold {threshold})");
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("REGRESSION against {path} (threshold {threshold}):");
    for r in &regressions {
        eprintln!("  {}: {:.3} -> {:.3}", r.metric, r.baseline, r.current);
    }
    Ok(ExitCode::from(1))
}

fn cmd_report(flags: &Flags) -> Result<ExitCode> {
    let path = flags.get("in").context("report needs --in report.json")?;
    let report = Report::load(Path::new(path))?;
    let baseline = flags
        .get("compare")
        .map(|p| Report::load(Path::new(p)))
        .transpose()?;
    print!("{}", to_markdown(&report, baseline.as_ref()));
    check_baseline(&report, flags)
}

async fn cmd_bench(flags: &Flags) -> Result<ExitCode> {
    let provider_name = flags.get("provider").unwrap_or("fake");
    let provider = providers::build(provider_name)?;
    let model = match flags.get("model") {
        Some(m) => m.to_owned(),
        None if provider.is_fake() => "fake-1".to_owned(),
        None => bail!("--model is required for provider {provider_name}"),
    };
    let dir = flags
        .get("dataset-dir")
        .map_or_else(dataset::default_dir, PathBuf::from);
    let ids = flags.all("meeting");
    let mut meetings = dataset::load_dir(&dir, &ids)?;
    if ids.is_empty() {
        meetings.truncate(1);
    }
    let sessions = flags
        .get("sessions")
        .unwrap_or("1,10,50")
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .with_context(|| format!("--sessions: bad count `{s}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let dist = |k: &str, d: &str| bench::Dist::parse(flags.get(k).unwrap_or(d));
    let opts = bench::BenchOptions {
        provider,
        model,
        meetings,
        sessions,
        speed: flags.number::<f64>("speed")?.unwrap_or(1.0),
        stagger_ms: flags.number::<i64>("stagger-ms")?.unwrap_or(3000),
        max_pipelines: flags.number::<usize>("max-pipelines")?.unwrap_or(8),
        decide: dist("decide-ms", "300:100")?,
        generate: dist("generate-ms", "700:200")?,
        seed: flags.number::<u64>("seed")?.unwrap_or(1),
        suggestion_overrides: flags.json_file("suggestion-config")?,
        redis_url: std::env::var("REDIS_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| flags.get("redis-url").map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_REDIS_URL.to_owned()),
    };
    let report = bench::run_bench(&opts).await?;
    if let Some(out) = flags.get("out") {
        report.save(Path::new(out))?;
        eprintln!("bench report written to {out}");
    }
    let md = bench::to_markdown(&report);
    if let Some(p) = flags.get("md-out") {
        if let Some(dir) = Path::new(p).parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, &md).with_context(|| format!("writing {p}"))?;
    }
    print!("{md}");
    if let Some(target) = flags.number::<f64>("target-answer-p95-ms")? {
        let mut ok = true;
        for l in &report.levels {
            let pass = l.answer.n > 0 && l.answer.p95_ms <= target;
            ok &= pass;
            eprintln!(
                "{} sessions: answer p95 {:.0} ms vs target {target:.0} ms: {}",
                l.sessions,
                l.answer.p95_ms,
                if pass { "PASS" } else { "FAIL" }
            );
        }
        if !ok {
            return Ok(ExitCode::from(1));
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn cmd_run(flags: &Flags) -> Result<ExitCode> {
    let suite = Suite::parse(flags.get("suite").unwrap_or("all"))?;
    let provider_name = flags.get("provider").unwrap_or("fake");
    let provider = providers::build(provider_name)?;
    let model = match flags.get("model") {
        Some(m) => m.to_owned(),
        None if provider.is_fake() => "fake-1".to_owned(),
        None => bail!("--model is required for provider {provider_name}"),
    };
    let judge = match (flags.get("judge-provider"), flags.get("judge-model")) {
        (None, None) => None,
        (Some(p), Some(m)) => Some(JudgeSetup {
            provider_name: p.to_owned(),
            model: m.to_owned(),
            spec: providers::build(p)?,
        }),
        _ => bail!("--judge-provider and --judge-model go together"),
    };
    let dir = flags
        .get("dataset-dir")
        .map_or_else(dataset::default_dir, PathBuf::from);
    let meetings = dataset::load_dir(&dir, &flags.all("meeting"))?;
    let price = match (
        flags.number::<f64>("price-in")?,
        flags.number::<f64>("price-out")?,
    ) {
        (Some(i), Some(o)) => Some(Price {
            input_per_m: i,
            output_per_m: o,
        }),
        (None, None) => None,
        _ => bail!("--price-in and --price-out go together"),
    };
    let env_or = |flag: &str, var: &str, default: &str| {
        flags
            .get(flag)
            .map(str::to_owned)
            .or_else(|| std::env::var(var).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| default.to_owned())
    };
    let opts = RunOptions {
        suite,
        provider,
        model,
        judge,
        meetings,
        tolerance: flags.number::<usize>("tolerance")?.unwrap_or(1),
        suggestion_overrides: flags.json_file("suggestion-config")?,
        summary_overrides: flags.json_file("summary-config")?,
        price,
        redis_url: env_or("redis-url", "REDIS_URL", DEFAULT_REDIS_URL),
        database_url: env_or("database-url", "DATABASE_URL", DEFAULT_DATABASE_URL),
    };
    let report = run(&opts).await?;
    if let Some(out) = flags.get("out") {
        report.save(Path::new(out))?;
        eprintln!("report written to {out}");
    }
    let baseline = flags
        .get("compare")
        .map(|p| Report::load(Path::new(p)))
        .transpose()?;
    print!("{}", to_markdown(&report, baseline.as_ref()));
    let missed = misses(&report);
    if !missed.is_empty() {
        println!(
            "\n<details><summary>{} expectations not met</summary>\n",
            missed.len()
        );
        for l in missed {
            println!("- {l}");
        }
        println!("\n</details>");
    }
    check_baseline(&report, flags)
}
