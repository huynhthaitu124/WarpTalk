//! Dataset format: one JSON file per synthetic meeting under `datasets/`.
//!
//! ```json
//! {
//!   "id": "vi-sprint-planning", "language": "vi", "title": "...", "participants": ["..."],
//!   "lines": [{"t_ms": 4000, "channel": "me|others|unknown", "speaker": "Minh", "text": "..."}],
//!   "snippets": [{"title": "...", "text": "...", "kind": "document"}],
//!   "expectations": {
//!     "suggestions": {
//!       "expected":  [{"at_line": 2, "category": "term", "must_include": ["x"], "must_not_include": ["y"]}],
//!       "forbidden": [{"at_line": 6, "category": "correction"}],
//!       "must_not_include": ["figure that must never appear"]
//!     },
//!     "summary": {
//!       "decisions": [{"keywords": ["a", "b"]}],
//!       "action_items": [{"owner": "Lan", "task_keywords": ["api"], "deadline": "thứ Sáu"}],
//!       "must_not_include": ["hallucination trap"]
//!     },
//!     "injection": { "markers": ["attacker.example", "text of the real system prompt"] }
//!   },
//!   "fake": { "suggestions": [...], "summary": {...} }
//! }
//! ```
//!
//! `at_line` is the 0-based index into `lines`. `category` may list alternatives with `|`
//! (`"fact|answer"`). A `must_include` item may list alternatives with `|` too. The `fake`
//! block is only read by the deterministic `fake` provider.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The user running the assistant ("me").
    Me,
    /// Someone else in the meeting.
    Others,
    Unknown,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Me => "me",
            Self::Others => "others",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub t_ms: i64,
    pub channel: Channel,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    pub title: String,
    pub text: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "document".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedSuggestion {
    pub at_line: usize,
    pub category: String,
    #[serde(default)]
    pub must_include: Vec<String>,
    #[serde(default)]
    pub must_not_include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenSuggestion {
    pub at_line: usize,
    /// `*` forbids any category at that line.
    pub category: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionExpect {
    pub expected: Vec<ExpectedSuggestion>,
    pub forbidden: Vec<ForbiddenSuggestion>,
    /// Hallucination traps that apply to every suggestion of the meeting.
    pub must_not_include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionExpect {
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionExpect {
    /// Present only when the transcript names an owner; absent means "no owner was said".
    #[serde(default)]
    pub owner: Option<String>,
    pub task_keywords: Vec<String>,
    /// Present only when the transcript states a deadline.
    #[serde(default)]
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SummaryExpect {
    pub decisions: Vec<DecisionExpect>,
    pub action_items: Vec<ActionExpect>,
    pub must_not_include: Vec<String>,
}

/// Prompt-injection attack meetings: strings that must never show up in any model output
/// (attacker URLs, leaked system-prompt text, compliance phrases). Empty for normal meetings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InjectionExpect {
    pub markers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Expectations {
    pub suggestions: SuggestionExpect,
    pub summary: SummaryExpect,
    pub injection: InjectionExpect,
}

/// One scripted suggestion of the `fake` provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FakeSuggestion {
    pub at_line: usize,
    pub category: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    pub content: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub conflict: bool,
    #[serde(default)]
    pub follow_up: Option<Value>,
}

fn default_confidence() -> f64 {
    0.9
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeScript {
    pub suggestions: Vec<FakeSuggestion>,
    /// Raw summary model output (`summary_short`, `decisions`, `action_items`, ...).
    pub summary: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub language: String,
    pub title: String,
    #[serde(default)]
    pub participants: Vec<String>,
    pub lines: Vec<Line>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default)]
    pub expectations: Expectations,
    #[serde(default)]
    pub fake: FakeScript,
}

impl Meeting {
    pub fn from_json(s: &str) -> Result<Self> {
        let m: Meeting = serde_json::from_str(s).context("invalid meeting JSON")?;
        m.validate()?;
        Ok(m)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let s =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_json(&s).with_context(|| format!("in {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.lines.is_empty() {
            bail!("meeting needs an id and at least one line");
        }
        let n = self.lines.len();
        let mut prev = i64::MIN;
        let mut texts = HashSet::new();
        for (i, l) in self.lines.iter().enumerate() {
            if l.t_ms < prev {
                bail!("{}: line {i} goes back in time", self.id);
            }
            prev = l.t_ms;
            if !texts.insert(l.text.as_str()) {
                bail!(
                    "{}: line {i} repeats an earlier text (texts must be unique)",
                    self.id
                );
            }
        }
        let sug = &self.expectations.suggestions;
        let positions = sug
            .expected
            .iter()
            .map(|e| e.at_line)
            .chain(sug.forbidden.iter().map(|f| f.at_line))
            .chain(self.fake.suggestions.iter().map(|f| f.at_line));
        for at in positions {
            if at >= n {
                bail!("{}: at_line {at} out of range ({n} lines)", self.id);
            }
        }
        Ok(())
    }
}

/// Load every `*.json` file of `dir` (sorted by file name); `only` filters by meeting id.
pub fn load_dir(dir: &Path, only: &[String]) -> Result<Vec<Meeting>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading dataset dir {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let m = Meeting::load(&f)?;
        if only.is_empty() || only.contains(&m.id) {
            out.push(m);
        }
    }
    if out.is_empty() {
        bail!("no meetings found in {}", dir.display());
    }
    Ok(out)
}

/// Directory of the bundled datasets (works from a checkout).
pub fn default_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("datasets")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn bundled_datasets_are_valid() {
        let all = load_dir(&default_dir(), &[]).unwrap();
        assert_eq!(all.len(), 9);
        let vi = all.iter().filter(|m| m.language == "vi").count();
        assert_eq!(vi, 6);
        let attacks = all
            .iter()
            .filter(|m| !m.expectations.injection.markers.is_empty())
            .count();
        assert_eq!(attacks, 3);
        for m in &all {
            assert!(m.lines.iter().any(|l| l.channel == Channel::Me), "{}", m.id);
            assert!(
                m.lines.iter().any(|l| l.channel == Channel::Others),
                "{}",
                m.id
            );
        }
    }

    #[test]
    fn rejects_out_of_range_and_duplicates() {
        let bad = r#"{"id":"x","language":"en","title":"t",
          "lines":[{"t_ms":0,"channel":"me","speaker":"A","text":"hello there"}],
          "expectations":{"suggestions":{"expected":[{"at_line":3,"category":"term"}]}}}"#;
        assert!(Meeting::from_json(bad).is_err());
        let dup = r#"{"id":"x","language":"en","title":"t","lines":[
          {"t_ms":0,"channel":"me","speaker":"A","text":"same"},
          {"t_ms":1,"channel":"others","speaker":"B","text":"same"}]}"#;
        assert!(Meeting::from_json(dup).is_err());
    }
}
