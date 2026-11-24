//! Command line: `run` and `report`. Hand-rolled parsing (no extra dependencies).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::cost::Price;
use crate::dataset;
use crate::harness::{
    DEFAULT_DATABASE_URL, DEFAULT_REDIS_URL, JudgeSetup, RunOptions, Suite, run,
};
use crate::providers;
use crate::report::{Report, compare, misses, to_markdown};

pub const USAGE: &str = "\
warptalk-ai-eval: offline evaluation of the AI core

USAGE
  warptalk-ai-eval run [OPTIONS]
  warptalk-ai-eval report --in report.json [--compare baseline.json] [--threshold 0.03]

run OPTIONS
  --suite suggestions|summary|all     default all
  --provider fake|openai|anthropic|gemini   default fake (keys from OPENAI_API_KEY,
                                      ANTHROPIC_API_KEY, GEMINI_API_KEY)
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
            .map(|s| s.parse::<T>().map_err(|_| anyhow::anyhow!("--{k}: bad number `{s}`")))
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
        eprintln!(
            "  {}: {:.3} -> {:.3}",
            r.metric, r.baseline, r.current
        );
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
        println!("\n<details><summary>{} expectations not met</summary>\n", missed.len());
        for l in missed {
            println!("- {l}");
        }
        println!("\n</details>");
    }
    check_baseline(&report, flags)
}
