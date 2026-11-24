//! Report types, markdown rendering and baseline comparison.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::judge::JudgeStats;
use crate::recorder::{LatencyStats, UsageTotals};
use crate::scoring::{
    CategoryCounts, ExpectedOutcome, ExpectedStatus, Published, SuggestionMetrics, SummaryMetrics,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeInfo {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryReport {
    pub expected: u64,
    pub published: u64,
    pub true_positives: u64,
    pub precision: f64,
    pub recall: f64,
}

impl From<&CategoryCounts> for CategoryReport {
    fn from(c: &CategoryCounts) -> Self {
        Self {
            expected: c.expected,
            published: c.published,
            true_positives: c.true_positives,
            precision: c.precision(),
            recall: c.recall(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingSuggestions {
    pub id: String,
    pub metrics: SuggestionMetrics,
    pub expected: Vec<ExpectedOutcome>,
    pub published: Vec<Published>,
    pub hallucinations: Vec<(usize, String)>,
    pub forbidden_hits: Vec<(usize, String)>,
    /// Outcome label of every replayed line that did not publish, counted.
    pub outcomes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionsReport {
    pub metrics: SuggestionMetrics,
    pub by_category: BTreeMap<String, CategoryReport>,
    /// Keyed by call purpose (`suggestion_decide`, `suggestion_generate`).
    pub latency: BTreeMap<String, LatencyStats>,
    pub usage: UsageTotals,
    pub judge: Option<JudgeStats>,
    pub outcomes: BTreeMap<String, u64>,
    pub meetings: Vec<MeetingSuggestions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub id: String,
    pub metrics: SummaryMetrics,
    /// Why generation failed, when it did.
    pub error: Option<String>,
    pub missed_decisions: Vec<String>,
    pub missed_actions: Vec<String>,
    pub wrong_owner: Vec<String>,
    pub wrong_deadline: Vec<String>,
    pub traps_hit: Vec<String>,
    pub judge_score: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryReport {
    pub metrics: SummaryMetrics,
    pub latency: LatencyStats,
    pub usage: UsageTotals,
    pub judge: Option<JudgeStats>,
    pub meetings: Vec<MeetingSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub created_at: String,
    pub suite: String,
    pub provider: String,
    pub model: String,
    pub judge: Option<JudgeInfo>,
    pub line_tolerance: usize,
    /// True when latencies are derived from output size (fake provider), not measured.
    pub latency_simulated: bool,
    pub suggestions: Option<SuggestionsReport>,
    pub summary: Option<SummaryReport>,
}

impl Report {
    pub fn load(path: &Path) -> Result<Self> {
        let s =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))
    }
}

// ------------------------------------------------------------------ comparison

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    HigherIsBetter,
    LowerIsBetter,
}

/// Quality metrics compared against a baseline (latency and cost are reported, never gated).
pub fn quality_metrics(r: &Report) -> Vec<(String, f64, Direction)> {
    use Direction::{HigherIsBetter as Up, LowerIsBetter as Down};
    let mut v = Vec::new();
    if let Some(s) = &r.suggestions {
        let m = &s.metrics;
        v.push(("suggestions.precision".into(), m.precision, Up));
        v.push(("suggestions.recall".into(), m.recall, Up));
        v.push(("suggestions.f1".into(), m.f1, Up));
        v.push((
            "suggestions.forbidden_hit_rate".into(),
            m.forbidden_hit_rate,
            Down,
        ));
        v.push((
            "suggestions.hallucination_rate".into(),
            m.hallucination_rate,
            Down,
        ));
        if let Some(j) = s.judge.as_ref().filter(|j| j.judged > 0) {
            v.push((
                "suggestions.faithfulness".into(),
                j.faithfulness_mean / 5.0,
                Up,
            ));
        }
    }
    if let Some(s) = &r.summary {
        let m = &s.metrics;
        v.push(("summary.decision_recall".into(), m.decision_recall, Up));
        v.push(("summary.action_recall".into(), m.action_recall, Up));
        v.push(("summary.action_precision".into(), m.action_precision, Up));
        v.push(("summary.owner_accuracy".into(), m.owner_accuracy, Up));
        v.push(("summary.deadline_accuracy".into(), m.deadline_accuracy, Up));
        v.push(("summary.trap_hit_rate".into(), m.trap_hit_rate, Down));
        if let Some(j) = s.judge.as_ref().filter(|j| j.judged > 0) {
            v.push(("summary.faithfulness".into(), j.faithfulness_mean / 5.0, Up));
        }
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
}

/// Metrics present in both reports that got worse by more than `threshold` (absolute, on the
/// 0..1 scale of each metric).
pub fn compare(current: &Report, baseline: &Report, threshold: f64) -> Vec<Regression> {
    let base: BTreeMap<String, (f64, Direction)> = quality_metrics(baseline)
        .into_iter()
        .map(|(k, v, d)| (k, (v, d)))
        .collect();
    quality_metrics(current)
        .into_iter()
        .filter_map(|(k, cur, dir)| {
            let (b, _) = base.get(&k).copied()?;
            let worse = match dir {
                Direction::HigherIsBetter => b - cur,
                Direction::LowerIsBetter => cur - b,
            };
            (worse > threshold + 1e-9).then_some(Regression {
                metric: k,
                baseline: b,
                current: cur,
            })
        })
        .collect()
}

// ------------------------------------------------------------------ markdown

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn delta(cur: f64, base: Option<f64>) -> String {
    match base {
        Some(b) => format!("{:+.1} pt", (cur - b) * 100.0),
        None => "-".into(),
    }
}

fn base_value(base: Option<&Report>, key: &str) -> Option<f64> {
    base.and_then(|b| {
        quality_metrics(b)
            .into_iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, v, _)| v)
    })
}

fn latency_row(out: &mut String, name: &str, l: &LatencyStats) {
    let _ = writeln!(
        out,
        "| {name} | {} | {:.0} | {:.0} | {:.0} | {} |",
        l.calls, l.mean_ms, l.p50_ms, l.p95_ms, l.failed
    );
}

fn usage_line(out: &mut String, u: &UsageTotals) {
    let cost = u
        .cost_usd
        .map_or("n/a (unknown model price)".to_string(), |c| {
            format!("${c:.4}")
        });
    let _ = writeln!(
        out,
        "\nTokens: {} in / {} out over {} calls. Estimated cost: {cost}.",
        u.input_tokens, u.output_tokens, u.calls
    );
}

/// Markdown summary of a report; with `baseline`, a delta column is added.
pub fn to_markdown(r: &Report, baseline: Option<&Report>) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# AI eval report\n");
    let _ = writeln!(
        o,
        "- suite: `{}`\n- provider / model: `{}` / `{}`\n- judge: {}\n- created: {}\n- line tolerance: {}",
        r.suite,
        r.provider,
        r.model,
        r.judge.as_ref().map_or("none".to_string(), |j| format!(
            "`{}` / `{}`",
            j.provider, j.model
        )),
        r.created_at,
        r.line_tolerance
    );
    if baseline.is_some() {
        let _ = writeln!(o, "- compared with a baseline (delta column)");
    }
    let row = |o: &mut String, label: &str, key: &str, cur: f64| {
        let b = base_value(baseline, key);
        let _ = writeln!(
            o,
            "| {label} | {} | {} | {} |",
            pct(cur),
            b.map_or("-".into(), pct),
            delta(cur, b)
        );
    };

    if let Some(s) = &r.suggestions {
        let m = &s.metrics;
        let _ = writeln!(o, "\n## Suggestions\n");
        let _ = writeln!(
            o,
            "| metric | value | baseline | delta |\n|---|---|---|---|"
        );
        row(&mut o, "precision", "suggestions.precision", m.precision);
        row(&mut o, "recall", "suggestions.recall", m.recall);
        row(&mut o, "F1", "suggestions.f1", m.f1);
        row(
            &mut o,
            "forbidden-hit rate (lower is better)",
            "suggestions.forbidden_hit_rate",
            m.forbidden_hit_rate,
        );
        row(
            &mut o,
            "hallucination rate (lower is better)",
            "suggestions.hallucination_rate",
            m.hallucination_rate,
        );
        if let Some(j) = s.judge.as_ref().filter(|j| j.judged > 0) {
            let _ = writeln!(
                o,
                "| judge faithfulness (1-5) | {:.2} | - | - |",
                j.faithfulness_mean
            );
        }
        let _ = writeln!(
            o,
            "\nExpected {}, published {}, correct {}; forbidden hits {}/{}; hallucination hits {}.",
            m.expected,
            m.published,
            m.true_positives,
            m.forbidden_hits,
            m.forbidden_total,
            m.hallucination_hits
        );
        let _ = writeln!(
            o,
            "\n| category | expected | published | correct | precision | recall |\n|---|---|---|---|---|---|"
        );
        for (k, c) in &s.by_category {
            let _ = writeln!(
                o,
                "| {k} | {} | {} | {} | {} | {} |",
                c.expected,
                c.published,
                c.true_positives,
                pct(c.precision),
                pct(c.recall)
            );
        }
        let _ = writeln!(
            o,
            "\n| meeting | expected | published | correct | precision | recall | forbidden | traps |\n|---|---|---|---|---|---|---|---|"
        );
        for mt in &s.meetings {
            let x = &mt.metrics;
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {}/{} | {} |",
                mt.id,
                x.expected,
                x.published,
                x.true_positives,
                pct(x.precision),
                pct(x.recall),
                x.forbidden_hits,
                x.forbidden_total,
                x.hallucination_hits
            );
        }
        let _ = writeln!(
            o,
            "\n| call | n | mean ms | p50 ms | p95 ms | failed |\n|---|---|---|---|---|---|"
        );
        for (k, l) in &s.latency {
            latency_row(&mut o, k, l);
        }
        if r.latency_simulated {
            let _ = writeln!(o, "\nLatency is simulated for the fake provider.");
        }
        usage_line(&mut o, &s.usage);
    }

    if let Some(s) = &r.summary {
        let m = &s.metrics;
        let _ = writeln!(o, "\n## Summary\n");
        let _ = writeln!(
            o,
            "| metric | value | baseline | delta |\n|---|---|---|---|"
        );
        row(
            &mut o,
            "decision recall",
            "summary.decision_recall",
            m.decision_recall,
        );
        row(
            &mut o,
            "action item recall",
            "summary.action_recall",
            m.action_recall,
        );
        row(
            &mut o,
            "action item precision",
            "summary.action_precision",
            m.action_precision,
        );
        row(
            &mut o,
            "owner accuracy",
            "summary.owner_accuracy",
            m.owner_accuracy,
        );
        row(
            &mut o,
            "deadline accuracy",
            "summary.deadline_accuracy",
            m.deadline_accuracy,
        );
        row(
            &mut o,
            "trap-hit rate (lower is better)",
            "summary.trap_hit_rate",
            m.trap_hit_rate,
        );
        if let Some(j) = s.judge.as_ref().filter(|j| j.judged > 0) {
            let _ = writeln!(
                o,
                "| judge faithfulness (1-5) | {:.2} | - | - |",
                j.faithfulness_mean
            );
        }
        let c = &m.counts;
        let _ = writeln!(
            o,
            "\nDecisions {}/{}, action items {}/{} ({} produced), owner invented {}, deadline invented {}, traps {}/{}, failed meetings {}.",
            c.decisions_hit,
            c.decisions_expected,
            c.actions_hit,
            c.actions_expected,
            c.actions_produced,
            c.owner_invented,
            c.deadline_invented,
            c.trap_hits,
            c.traps_total,
            c.failed
        );
        let _ = writeln!(
            o,
            "\n| meeting | decisions | actions | owner | deadline | traps | judge |\n|---|---|---|---|---|---|---|"
        );
        for mt in &s.meetings {
            let c = &mt.metrics.counts;
            let _ = writeln!(
                o,
                "| {} | {}/{} | {}/{} | {}/{} | {}/{} | {}/{} | {} |",
                mt.id,
                c.decisions_hit,
                c.decisions_expected,
                c.actions_hit,
                c.actions_expected,
                c.owner_correct,
                c.owner_checked,
                c.deadline_correct,
                c.deadline_checked,
                c.trap_hits,
                c.traps_total,
                mt.error
                    .clone()
                    .or_else(|| mt.judge_score.map(|j| j.to_string()))
                    .unwrap_or_else(|| "-".into())
            );
        }
        let _ = writeln!(
            o,
            "\n| call | n | mean ms | p50 ms | p95 ms | failed |\n|---|---|---|---|---|---|"
        );
        latency_row(&mut o, "summary", &s.latency);
        usage_line(&mut o, &s.usage);
    }
    o
}

/// One line per missed expectation, for a quick look at what went wrong.
pub fn misses(r: &Report) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(s) = &r.suggestions {
        for m in &s.meetings {
            for e in &m.expected {
                if e.status != ExpectedStatus::Hit {
                    v.push(format!(
                        "{}: line {} `{}` {:?}",
                        m.id, e.at_line, e.category, e.status
                    ));
                }
            }
        }
    }
    v
}
