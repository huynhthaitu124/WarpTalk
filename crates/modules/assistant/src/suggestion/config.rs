//! Extended suggestion configuration (`SuggestionConfig`).
//!
//! Stored in the `suggestion` settings namespace ([`warptalk_settings::NS_SUGGESTION`]) and
//! resolved per room through `SettingsStore` (platform < workspace < room, deep-merged). It is a
//! strict superset of [`warptalk_settings::namespaces::SuggestionSettings`]: every base field keeps
//! its name and type, so documents written for the base type stay valid. Call [`register`] once at
//! start-up so `SettingsStore::put` validates the extended document instead of the base one.
//!
//! Nothing here is a constant of the pipeline: defaults reproduce the upstream behaviour
//! (spec 09 section 12) and every knob can be overridden per platform / workspace / room.
//!
//! # JSON schema (all keys optional; omitted keys inherit lower scopes, then defaults)
//!
//! ```text
//! {
//!   "enabled": true,                        // deploy switch (gate 1)
//!   "decide_model": "gpt-4o-mini", "generate_model": "gpt-4.1",   // per-stage model (a BYO resolver may override)
//!   "decide_max_tokens": 64, "generate_max_tokens": 200,          // per language for generate
//!   "temperature": 0.2, "request_timeout_seconds": 8.0,
//!   "min_confidence": 0.55,                 // global floor; per-category floor in category_rules
//!   "sensitivity": 0.5, "sensitivity_span": 0.4,   // dial: floor -= (sensitivity - 0.5) * span
//!   "cooldown_seconds": 20, "max_per_meeting": 30, "state_ttl_seconds": 14400,
//!   "budget": { "max_per_workspace": 0, "workspace_window_seconds": 86400 },   // 0 = unlimited
//!   "min_words": 4, "min_question_words": 2, "min_stt_confidence": -0.5, "stt_unknown_sentinel": -1.0,
//!   "window_size": 8,                       // turns
//!   "window": { "max_age_seconds": 0, "max_tokens": 0, "chars_per_token": 4.0 },   // 0 = no limit
//!   "max_suggestion_chars": 140, "max_detail_chars": 600,
//!   "categories": [ { "id": "term", "enabled": false } ],        // toggles (unlisted = enabled)
//!   "category_rules": { "<id>": {                                 // overlay on the built-in rules
//!       "description": "...", "contract": "...", "min_confidence": 0.7,
//!       "retrieval": "never|optional|required", "kind": "hint|follow_up",
//!       "severity": "info|warning|critical", "ttl_seconds": 120, "wire_name": "action",
//!       "allow_general_knowledge": false, "enabled": true,
//!       "actions": [ { "kind": "create_action_item", "label": "Create task" } ] } },
//!   "heuristics": { "<lang or *>": { "question_marks": [], "question_words": [], "filler_words": [] } },
//!   "prompts": { "<name>": "template text" },                     // base-compatible overrides
//!   "prompt_templates": { "<name>": { "version": "v2", "text": "..." } },   // versioned overrides
//!   "language": { "mode": "listener|speaker|fixed", "fixed": "en", "max_reader_languages": 4,
//!                 "scale_tokens_per_language": true, "names": { "en": "English" } },
//!   "hygiene": { "blocked_phrases": [], "redact_pii": true, "digit_run_min": 10,
//!                "scrub_input": true, "min_content_chars": 1 },
//!   "dedup": { "window_seconds": 900 },                           // 0 disables
//!   "retrieval": { "top_k": 3, "timeout_ms": 3000, "max_snippet_chars": 1200 },
//!   "follow_up": { "verify_in_transcript": true, "task_max_chars": 1000,
//!                  "owner_max_chars": 200, "due_max_chars": 100 },
//!   "delivery": { "stream_maxlen": 1000, "followup_stream_maxlen": 1000 },
//!   "consent": { "cache_seconds": 60 },
//!   "prompt_cache": true
//! }
//! ```
//!
//! Prompt names: `decide`, `decide_user`, `generate`, `generate_user`, `untrusted_rule`,
//! `translations_rule`, `general_knowledge_rule`, `follow_up_rule`. Variables are written
//! `{{name}}`; see [`crate::suggestion::prompt::KNOWN_VARS`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use warptalk_settings::{NS_SUGGESTION, SettingsStore, SuggestionCategory, Validate, check_range};

use super::prompt;

/// Register [`SuggestionConfig`] as the validator of the `suggestion` namespace (replaces the
/// base `SuggestionSettings` validator registered by `register_defaults`).
pub fn register(store: &SettingsStore) {
    store.register::<SuggestionConfig>(NS_SUGGESTION);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    /// Never query the knowledge port for this category.
    #[default]
    Never,
    /// Query; proceed without documents when nothing is found.
    Optional,
    /// Query; stay silent when nothing grounded is found ("fact needs docs").
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CategoryKind {
    #[default]
    Hint,
    /// Produces a structured `FollowUp` published on the follow-up stream.
    FollowUp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub kind: String,
    #[serde(default)]
    pub label: String,
}

/// Overlay on a built-in category rule; every field is optional.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryRule {
    pub enabled: Option<bool>,
    pub description: Option<String>,
    pub contract: Option<String>,
    pub min_confidence: Option<f32>,
    pub retrieval: Option<RetrievalMode>,
    pub kind: Option<CategoryKind>,
    pub severity: Option<String>,
    pub ttl_seconds: Option<u32>,
    /// Name sent on the wire in `category` (client compatibility), defaults to the id.
    pub wire_name: Option<String>,
    pub allow_general_knowledge: Option<bool>,
    pub actions: Option<Vec<ActionSpec>>,
}

/// A category after built-in rules, overlays and toggles are merged.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCategory {
    pub id: String,
    pub enabled: bool,
    pub description: String,
    pub contract: String,
    pub min_confidence: Option<f32>,
    pub retrieval: RetrievalMode,
    pub kind: CategoryKind,
    pub severity: String,
    pub ttl_seconds: u32,
    pub wire_name: String,
    pub allow_general_knowledge: bool,
    pub actions: Vec<ActionSpec>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeuristicLang {
    pub question_marks: Vec<String>,
    pub question_words: Vec<String>,
    /// A segment made only of these words is rejected by stage 0.
    pub filler_words: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Drop turns older than this many seconds (0 = no age limit).
    pub max_age_seconds: u32,
    /// Drop oldest turns until the estimated token total fits (0 = no token limit).
    pub max_tokens: u32,
    pub chars_per_token: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            max_age_seconds: 0,
            max_tokens: 0,
            chars_per_token: 4.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Suggestions per workspace per `workspace_window_seconds` (0 = unlimited).
    pub max_per_workspace: u32,
    pub workspace_window_seconds: u32,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_per_workspace: 0,
            workspace_window_seconds: 86_400,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LanguageMode {
    /// Hint in the speaker's language plus one copy per reader (listener) language.
    #[default]
    Listener,
    /// Hint in the speaker's language only.
    Speaker,
    /// Always write in `fixed`.
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageConfig {
    pub mode: LanguageMode,
    pub fixed: String,
    pub max_reader_languages: u32,
    /// Multiply the generate token cap by `1 + languages`.
    pub scale_tokens_per_language: bool,
    /// Display names used inside the translation instruction (overlay on the built-ins).
    pub names: BTreeMap<String, String>,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            mode: LanguageMode::Listener,
            fixed: "en".into(),
            max_reader_languages: 4,
            scale_tokens_per_language: true,
            names: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HygieneConfig {
    /// Case-insensitive phrases; model output containing one is discarded.
    pub blocked_phrases: Vec<String>,
    pub redact_pii: bool,
    /// Digit runs of at least this many digits are masked (0 disables number masking).
    pub digit_run_min: u32,
    /// Scrub PII from transcript text before it is sent to the provider (spec 09 section 15).
    pub scrub_input: bool,
    pub min_content_chars: u32,
}

impl Default for HygieneConfig {
    fn default() -> Self {
        Self {
            blocked_phrases: [
                "ignore previous",
                "ignore all previous",
                "ignore the above",
                "disregard the above",
                "system prompt",
                "as an ai language model",
                "my instructions",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            redact_pii: true,
            digit_run_min: 10,
            scrub_input: true,
            min_content_chars: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DedupConfig {
    /// Identical normalised hints within this window are suppressed (0 disables).
    pub window_seconds: u32,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            window_seconds: 900,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    pub top_k: u32,
    pub timeout_ms: u32,
    pub max_snippet_chars: u32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            top_k: 3,
            timeout_ms: 3000,
            max_snippet_chars: 1200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowUpConfig {
    /// Owner and due text must appear in the transcript window, else they are dropped.
    pub verify_in_transcript: bool,
    pub task_max_chars: u32,
    pub owner_max_chars: u32,
    pub due_max_chars: u32,
}

impl Default for FollowUpConfig {
    fn default() -> Self {
        Self {
            verify_in_transcript: true,
            task_max_chars: 1000,
            owner_max_chars: 200,
            due_max_chars: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeliveryConfig {
    pub stream_maxlen: u32,
    pub followup_stream_maxlen: u32,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            stream_maxlen: 1000,
            followup_stream_maxlen: 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsentConfig {
    /// How long a room's consent decision is trusted by cached adapters.
    pub cache_seconds: u32,
}

impl Default for ConsentConfig {
    fn default() -> Self {
        Self { cache_seconds: 60 }
    }
}

/// See the module documentation for the JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionConfig {
    // ---- superset of SuggestionSettings (same names and types) ----
    pub enabled: bool,
    pub decide_model: String,
    pub generate_model: String,
    pub decide_max_tokens: u32,
    pub generate_max_tokens: u32,
    pub temperature: f32,
    pub request_timeout_seconds: f32,
    pub min_confidence: f32,
    pub cooldown_seconds: u32,
    pub max_per_meeting: u32,
    pub min_words: u32,
    pub min_question_words: u32,
    pub min_stt_confidence: f32,
    pub window_size: u32,
    pub max_suggestion_chars: u32,
    pub state_ttl_seconds: u32,
    pub categories: Vec<SuggestionCategory>,
    pub prompts: BTreeMap<String, String>,
    // ---- extensions ----
    pub stt_unknown_sentinel: f32,
    /// Cap on `detail` (the expanded evidence text).
    pub max_detail_chars: u32,
    pub sensitivity: f32,
    pub sensitivity_span: f32,
    pub budget: BudgetConfig,
    pub window: WindowConfig,
    pub category_rules: BTreeMap<String, CategoryRule>,
    pub heuristics: BTreeMap<String, HeuristicLang>,
    pub prompt_templates: BTreeMap<String, PromptTemplate>,
    pub language: LanguageConfig,
    pub hygiene: HygieneConfig,
    pub dedup: DedupConfig,
    pub retrieval: RetrievalConfig,
    pub follow_up: FollowUpConfig,
    pub delivery: DeliveryConfig,
    pub consent: ConsentConfig,
    pub prompt_cache: bool,
}

impl Default for SuggestionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            decide_model: "gpt-4o-mini".into(),
            generate_model: "gpt-4.1".into(),
            decide_max_tokens: 64,
            generate_max_tokens: 200,
            temperature: 0.2,
            request_timeout_seconds: 8.0,
            min_confidence: 0.55,
            cooldown_seconds: 20,
            max_per_meeting: 30,
            min_words: 4,
            min_question_words: 2,
            min_stt_confidence: -0.5,
            window_size: 8,
            max_suggestion_chars: 140,
            state_ttl_seconds: 14_400,
            categories: Vec::new(),
            prompts: BTreeMap::new(),
            stt_unknown_sentinel: -1.0,
            max_detail_chars: 600,
            sensitivity: 0.5,
            sensitivity_span: 0.4,
            budget: BudgetConfig::default(),
            window: WindowConfig::default(),
            category_rules: BTreeMap::new(),
            heuristics: BTreeMap::new(),
            prompt_templates: BTreeMap::new(),
            language: LanguageConfig::default(),
            hygiene: HygieneConfig::default(),
            dedup: DedupConfig::default(),
            retrieval: RetrievalConfig::default(),
            follow_up: FollowUpConfig::default(),
            delivery: DeliveryConfig::default(),
            consent: ConsentConfig::default(),
            prompt_cache: true,
        }
    }
}

const SEVERITIES: [&str; 3] = ["info", "warning", "critical"];

impl Validate for SuggestionConfig {
    fn validate(&self) -> Result<(), String> {
        check_range("min_confidence", self.min_confidence, 0.0, 1.0)?;
        check_range("sensitivity", self.sensitivity, 0.0, 1.0)?;
        check_range("sensitivity_span", self.sensitivity_span, 0.0, 1.0)?;
        check_range("cooldown_seconds", self.cooldown_seconds, 0, 600)?;
        check_range("max_per_meeting", self.max_per_meeting, 0, 200)?;
        check_range("min_words", self.min_words, 1, 20)?;
        check_range("min_question_words", self.min_question_words, 1, 20)?;
        check_range("min_stt_confidence", self.min_stt_confidence, -5.0, 0.0)?;
        check_range("temperature", self.temperature, 0.0, 2.0)?;
        check_range(
            "request_timeout_seconds",
            self.request_timeout_seconds,
            0.5,
            60.0,
        )?;
        check_range("window_size", self.window_size, 1, 100)?;
        check_range("max_suggestion_chars", self.max_suggestion_chars, 20, 2000)?;
        check_range("max_detail_chars", self.max_detail_chars, 0, 8000)?;
        check_range("decide_max_tokens", self.decide_max_tokens, 16, 4096)?;
        check_range("generate_max_tokens", self.generate_max_tokens, 32, 8192)?;
        check_range(
            "window.chars_per_token",
            self.window.chars_per_token,
            1.0,
            16.0,
        )?;
        check_range(
            "window.max_age_seconds",
            self.window.max_age_seconds,
            0,
            86_400,
        )?;
        check_range(
            "budget.workspace_window_seconds",
            self.budget.workspace_window_seconds,
            1,
            2_592_000,
        )?;
        check_range("hygiene.digit_run_min", self.hygiene.digit_run_min, 0, 40)?;
        check_range("dedup.window_seconds", self.dedup.window_seconds, 0, 86_400)?;
        check_range("retrieval.top_k", self.retrieval.top_k, 1, 20)?;
        check_range(
            "retrieval.timeout_ms",
            self.retrieval.timeout_ms,
            100,
            30_000,
        )?;
        check_range(
            "language.max_reader_languages",
            self.language.max_reader_languages,
            0,
            10,
        )?;
        if self.decide_model.trim().is_empty() || self.generate_model.trim().is_empty() {
            return Err("model names must not be empty".into());
        }
        if self.categories.iter().any(|c| c.id.trim().is_empty()) {
            return Err("category ids must not be empty".into());
        }
        if self.language.mode == LanguageMode::Fixed && self.language.fixed.trim().is_empty() {
            return Err("language.fixed must be set when language.mode is fixed".into());
        }
        let builtin = builtin_categories();
        for (id, rule) in &self.category_rules {
            if id.trim().is_empty() {
                return Err("category_rules keys must not be empty".into());
            }
            if let Some(c) = rule.min_confidence {
                check_range(&format!("category_rules.{id}.min_confidence"), c, 0.0, 1.0)?;
            }
            if let Some(s) = &rule.severity
                && !SEVERITIES.contains(&s.as_str())
            {
                return Err(format!(
                    "category_rules.{id}.severity must be one of {SEVERITIES:?}"
                ));
            }
            if !builtin.iter().any(|b| &b.id == id) {
                let ok = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
                if !ok(&rule.description) || !ok(&rule.contract) {
                    return Err(format!(
                        "new category `{id}` needs a description and a contract"
                    ));
                }
            }
        }
        for (lang, h) in &self.heuristics {
            if lang.trim().is_empty() {
                return Err("heuristics keys must not be empty".into());
            }
            if h.question_marks.iter().any(|m| m.is_empty()) {
                return Err(format!(
                    "heuristics.{lang}.question_marks has an empty entry"
                ));
            }
        }
        for (name, text) in self
            .prompts
            .iter()
            .chain(self.prompt_templates.iter().map(|(n, t)| (n, &t.text)))
        {
            if !prompt::TEMPLATE_NAMES.contains(&name.as_str()) {
                return Err(format!("unknown prompt template `{name}`"));
            }
            if text.trim().is_empty() {
                return Err(format!("prompt template `{name}` is empty"));
            }
            for var in prompt::variables_in(text) {
                if !prompt::KNOWN_VARS.contains(&var.as_str()) {
                    return Err(format!(
                        "prompt template `{name}` uses unknown variable `{{{{{var}}}}}`"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct BuiltinCategory {
    id: String,
    #[serde(flatten)]
    rule: CategoryRule,
}

fn builtin_categories() -> &'static [ResolvedCategory] {
    static CELL: OnceLock<Vec<ResolvedCategory>> = OnceLock::new();
    CELL.get_or_init(|| {
        let raw: Vec<BuiltinCategory> =
            serde_json::from_str(include_str!("defaults/categories.json")).unwrap_or_default();
        raw.into_iter()
            .map(|b| apply_rule(&b.id, &b.rule, None))
            .collect()
    })
}

fn builtin_heuristics() -> &'static BTreeMap<String, HeuristicLang> {
    static CELL: OnceLock<BTreeMap<String, HeuristicLang>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(include_str!("defaults/heuristics.json")).unwrap_or_default()
    })
}

fn builtin_language_names() -> &'static BTreeMap<String, String> {
    static CELL: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(include_str!("defaults/languages.json")).unwrap_or_default()
    })
}

fn apply_rule(id: &str, rule: &CategoryRule, base: Option<&ResolvedCategory>) -> ResolvedCategory {
    let mut r = base.cloned().unwrap_or_else(|| ResolvedCategory {
        id: id.to_string(),
        enabled: true,
        description: String::new(),
        contract: String::new(),
        min_confidence: None,
        retrieval: RetrievalMode::Never,
        kind: CategoryKind::Hint,
        severity: "info".into(),
        ttl_seconds: 120,
        wire_name: id.to_string(),
        allow_general_knowledge: false,
        actions: Vec::new(),
    });
    if let Some(v) = rule.enabled {
        r.enabled = v;
    }
    if let Some(v) = &rule.description {
        r.description = v.clone();
    }
    if let Some(v) = &rule.contract {
        r.contract = v.clone();
    }
    if rule.min_confidence.is_some() {
        r.min_confidence = rule.min_confidence;
    }
    if let Some(v) = rule.retrieval {
        r.retrieval = v;
    }
    if let Some(v) = rule.kind {
        r.kind = v;
    }
    if let Some(v) = &rule.severity {
        r.severity = v.clone();
    }
    if let Some(v) = rule.ttl_seconds {
        r.ttl_seconds = v;
    }
    if let Some(v) = &rule.wire_name {
        r.wire_name = v.clone();
    }
    if let Some(v) = rule.allow_general_knowledge {
        r.allow_general_knowledge = v;
    }
    if let Some(v) = &rule.actions {
        r.actions = v.clone();
    }
    r
}

/// Lower-cased primary language subtag ("vi-VN" -> "vi").
pub fn primary_language(code: &str) -> String {
    code.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_lowercase()
}

impl SuggestionConfig {
    /// All categories (built-ins + config-defined), with overlays and toggles applied.
    /// Disabled ones are included with `enabled == false`.
    pub fn resolved_categories(&self) -> Vec<ResolvedCategory> {
        let mut out: Vec<ResolvedCategory> = builtin_categories()
            .iter()
            .map(|b| match self.category_rules.get(&b.id) {
                Some(rule) => apply_rule(&b.id, rule, Some(b)),
                None => b.clone(),
            })
            .collect();
        for (id, rule) in &self.category_rules {
            if !out.iter().any(|c| &c.id == id) {
                out.push(apply_rule(id, rule, None));
            }
        }
        for toggle in &self.categories {
            if let Some(c) = out.iter_mut().find(|c| c.id == toggle.id) {
                c.enabled = toggle.enabled;
            }
        }
        out
    }

    pub fn enabled_categories(&self) -> Vec<ResolvedCategory> {
        self.resolved_categories()
            .into_iter()
            .filter(|c| c.enabled)
            .collect()
    }

    /// Effective confidence floor for a category: its own floor (or the global one), shifted by
    /// the sensitivity dial. Higher sensitivity lowers the floor.
    pub fn confidence_floor(&self, category: &ResolvedCategory) -> f32 {
        let base = category.min_confidence.unwrap_or(self.min_confidence);
        (base - (self.sensitivity - 0.5) * self.sensitivity_span).clamp(0.0, 1.0)
    }

    /// Stage-0 word lists for a segment language: the `*` entry merged with the language entry.
    /// For each entry a non-empty configured list replaces the built-in list.
    pub fn heuristics_for(&self, language: &str) -> HeuristicLang {
        let pick = |key: &str| -> HeuristicLang {
            let builtin = builtin_heuristics().get(key).cloned().unwrap_or_default();
            let cfg = self.heuristics.get(key);
            let choose = |c: Option<&Vec<String>>, b: Vec<String>| match c {
                Some(c) if !c.is_empty() => c.clone(),
                _ => b,
            };
            HeuristicLang {
                question_marks: choose(cfg.map(|c| &c.question_marks), builtin.question_marks),
                question_words: choose(cfg.map(|c| &c.question_words), builtin.question_words),
                filler_words: choose(cfg.map(|c| &c.filler_words), builtin.filler_words),
            }
        };
        let mut out = pick("*");
        let lang = primary_language(language);
        if !lang.is_empty() && lang != "*" {
            let l = pick(&lang);
            merge_list(&mut out.question_marks, &l.question_marks);
            merge_list(&mut out.question_words, &l.question_words);
            merge_list(&mut out.filler_words, &l.filler_words);
        }
        out
    }

    pub fn language_name(&self, code: &str) -> String {
        let key = primary_language(code);
        self.language
            .names
            .get(&key)
            .or_else(|| builtin_language_names().get(&key))
            .cloned()
            .unwrap_or_else(|| code.to_string())
    }
}

fn merge_list(into: &mut Vec<String>, from: &[String]) {
    for item in from {
        if !into.contains(item) {
            into.push(item.clone());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_defaults_parse() {
        let cfg = SuggestionConfig::default();
        cfg.validate().unwrap();
        let ids: Vec<_> = cfg
            .resolved_categories()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(
            ids,
            [
                "clarification",
                "term",
                "consideration",
                "concept",
                "comparison",
                "correction",
                "fact",
                "action_item",
                "follow_up"
            ]
        );
        assert!(!builtin_heuristics().is_empty());
        assert!(!builtin_language_names().is_empty());
    }

    #[test]
    fn partial_document_inherits_defaults() {
        let cfg: SuggestionConfig =
            serde_json::from_value(json!({"min_confidence": 0.9, "window": {"max_tokens": 500}}))
                .unwrap();
        assert!((cfg.min_confidence - 0.9).abs() < 1e-6);
        assert_eq!(cfg.window.max_tokens, 500);
        assert!((cfg.window.chars_per_token - 4.0).abs() < 1e-6);
        assert_eq!(cfg.cooldown_seconds, 20);
    }

    #[test]
    fn overlay_and_toggle() {
        let cfg: SuggestionConfig = serde_json::from_value(json!({
            "categories": [{"id": "term", "enabled": false}],
            "category_rules": {"fact": {"min_confidence": 0.8}, "custom": {"description": "d", "contract": "c"}}
        }))
        .unwrap();
        cfg.validate().unwrap();
        let enabled: Vec<_> = cfg.enabled_categories().into_iter().map(|c| c.id).collect();
        assert!(!enabled.contains(&"term".to_string()));
        assert!(enabled.contains(&"custom".to_string()));
        let fact = cfg
            .resolved_categories()
            .into_iter()
            .find(|c| c.id == "fact")
            .unwrap();
        assert_eq!(fact.retrieval, RetrievalMode::Required);
        assert!((cfg.confidence_floor(&fact) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn sensitivity_shifts_floor() {
        let mut cfg = SuggestionConfig::default();
        let cat = cfg.enabled_categories().remove(0);
        let base = cfg.confidence_floor(&cat);
        cfg.sensitivity = 1.0;
        assert!(cfg.confidence_floor(&cat) < base);
        cfg.sensitivity = 0.0;
        assert!(cfg.confidence_floor(&cat) > base);
    }

    #[test]
    fn rejects_bad_values() {
        let bad = |v| {
            serde_json::from_value::<SuggestionConfig>(v)
                .unwrap()
                .validate()
        };
        assert!(bad(json!({"min_confidence": 2.0})).is_err());
        assert!(bad(json!({"prompts": {"decide": "x {{nope}}"}})).is_err());
        assert!(bad(json!({"prompts": {"bogus": "x"}})).is_err());
        assert!(bad(json!({"category_rules": {"new": {"description": "d"}}})).is_err());
        assert!(bad(json!({"category_rules": {"fact": {"severity": "loud"}}})).is_err());
        assert!(bad(json!({"prompts": {"decide": "ok {{max_chars}}"}})).is_ok());
    }

    #[test]
    fn heuristics_merge_by_language() {
        let cfg = SuggestionConfig::default();
        let vi = cfg.heuristics_for("vi-VN");
        assert!(vi.question_words.contains(&"gì".to_string()));
        assert!(vi.question_marks.contains(&"?".to_string()));
        let custom: SuggestionConfig =
            serde_json::from_value(json!({"heuristics": {"en": {"filler_words": ["zzz"]}}}))
                .unwrap();
        assert_eq!(
            custom.heuristics_for("en").filler_words,
            vec!["zzz".to_string()]
        );
    }
}
