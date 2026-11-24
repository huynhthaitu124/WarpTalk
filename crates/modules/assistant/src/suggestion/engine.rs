//! The suggestion pipeline (spec 09 sections 2, 3, 6, 7, 9, 10, 13).
//!
//! `admit` runs the cheap, synchronous-ish front half (gates 1-4, window, stage 0, cap, room
//! concurrency); `run` runs the model stages. `process` is both in sequence. Every failure path
//! ends in silence: no retries for decide, no redelivery.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use redis::aio::ConnectionManager;
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_bus::xadd;
use warptalk_contracts::events::{SYSTEM_SPEAKER, SttResult};
use warptalk_kernel::{Clock, RoomId, WorkspaceId};
use warptalk_llm::{CacheHint, JsonSchemaFormat, LlmError, Message, Request, Response, Usage};
use warptalk_telemetry::{
    Outcome as Metric, STAGE_OUTCOME_METRIC, StageTimer, record_stage_outcome,
};

use super::config::{
    CategoryKind, LanguageMode, ResolvedCategory, RetrievalMode, SuggestionConfig, primary_language,
};
use super::config_source::ConfigSource;
use super::hygiene;
use super::model::{
    AI_ASSISTANT_RESULTS, ASSISTANT_FOLLOWUPS, FollowUp, FollowUpEvent, Outcome, Role, SourceRef,
    SuggestionResult, Turn,
};
use super::ports::{
    ConsentPolicy, DataBoundary, GroundedSnippet, KnowledgeQuery, KnowledgeRetriever, LlmResolver,
    Purpose, ResolvedLlm, RoomDirectory, RoomSignals,
};
use super::prompt;
use super::slot::{SlotDenied, SlotParams, SuggestionState};
use super::stage0::{self, Stage0};
use super::window::{RollingWindow, estimate_tokens};
use crate::usage::{SharedUsageSink, UsageEvent};

const STAGE_DECIDE: &str = "suggestion_decide";
const STAGE_GENERATE: &str = "suggestion_generate";
const STAGE_SEGMENT: &str = "suggestion_segment";

#[derive(Clone)]
pub struct EngineDeps {
    pub rooms: Arc<dyn RoomDirectory>,
    pub signals: Arc<dyn RoomSignals>,
    pub consent: Arc<dyn ConsentPolicy>,
    pub llm: Arc<dyn LlmResolver>,
    pub knowledge: Arc<dyn KnowledgeRetriever>,
    pub config: Arc<dyn ConfigSource>,
    pub clock: Arc<dyn Clock>,
    pub state: SuggestionState,
    pub redis: ConnectionManager,
}

/// Holds a room's single pipeline slot until dropped.
pub struct InflightGuard {
    set: Arc<Mutex<HashSet<RoomId>>>,
    room: RoomId,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.room);
        }
    }
}

/// A segment that cleared the front half and holds its room's pipeline slot.
pub struct Admitted {
    room: RoomId,
    workspace: WorkspaceId,
    cfg: Arc<SuggestionConfig>,
    stt: SttResult,
    turn: Turn,
    context: Vec<Turn>,
    /// Another participant asks for a reply and the `answer` fast path may apply.
    answer_candidate: bool,
    _guard: InflightGuard,
}

impl Admitted {
    /// Role of the speaker of the admitted segment.
    pub fn role(&self) -> Role {
        self.turn.role
    }
}

pub enum Admission {
    Rejected(Outcome),
    Admitted(Box<Admitted>),
}

pub struct SuggestionEngine {
    deps: EngineDeps,
    usage: Option<SharedUsageSink>,
    windows: Mutex<HashMap<RoomId, RollingWindow>>,
    inflight: Arc<Mutex<HashSet<RoomId>>>,
}

#[derive(Debug)]
enum CallError {
    Timeout,
    Failed,
}

struct RequestSpec {
    model: String,
    system: String,
    user: String,
    schema: JsonSchemaFormat,
    max_tokens: u32,
    purpose: Purpose,
    version: String,
}

struct Decision {
    should_suggest: bool,
    category: String,
    confidence: f32,
    reason: String,
    tokens: u32,
}

struct Generated {
    content: String,
    detail: String,
    source: String,
    conflict: bool,
    translations: BTreeMap<String, (String, String)>,
    follow_up: Option<Value>,
    tokens: u32,
}

fn fin(outcome: Outcome) -> Outcome {
    metrics::counter!(STAGE_OUTCOME_METRIC, "stage" => STAGE_SEGMENT, "outcome" => outcome.label())
        .increment(1);
    tracing::debug!(outcome = outcome.label(), "suggestion segment finished");
    outcome
}

fn record_custom(stage: &'static str, label: &'static str) {
    metrics::counter!(STAGE_OUTCOME_METRIC, "stage" => stage, "outcome" => label).increment(1);
}

fn metric_for(e: &LlmError) -> Metric {
    match e {
        LlmError::Timeout => Metric::Timeout,
        LlmError::RateLimited { .. }
        | LlmError::Overloaded(_)
        | LlmError::Auth(_)
        | LlmError::QuotaOrBilling(_)
        | LlmError::Transport(_) => Metric::VendorError,
        _ => Metric::Error,
    }
}

/// First JSON object in `text` (tolerates code fences and prose around it).
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t)
        && v.is_object()
    {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&t[start..=end])
        .ok()
        .filter(Value::is_object)
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

impl SuggestionEngine {
    pub fn new(deps: EngineDeps) -> Self {
        Self {
            deps,
            usage: None,
            windows: Mutex::new(HashMap::new()),
            inflight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Publish `usage:events` for every LLM call (billing metering).
    pub fn with_usage(mut self, sink: SharedUsageSink) -> Self {
        self.usage = Some(sink);
        self
    }

    /// Fire-and-forget: metering must not add latency to the suggestion path or fail it.
    /// One event per (segment, stage), so reprocessing a segment is never charged twice.
    fn meter(&self, a: &Admitted, llm: &ResolvedLlm, model: &str, stage: &str, usage: &Usage) {
        let Some(sink) = self.usage.clone() else {
            return;
        };
        let (tin, tout) = UsageEvent::tokens_of(usage);
        let segment = a.stt.segment_id.parse::<Uuid>().ok();
        let event = UsageEvent::llm(
            format!("llm_suggestion:{}:{}:{stage}", a.room, a.stt.segment_id),
            "llm_suggestion",
            a.workspace.as_uuid(),
            a.room.as_uuid(),
            model.to_owned(),
            llm.boundary,
            tin,
            tout,
            segment,
        );
        tokio::spawn(async move { sink.emit(event).await });
    }

    /// Drop a finished room's window and watchers.
    pub fn forget_room(&self, room: RoomId) {
        if let Ok(mut w) = self.windows.lock() {
            w.remove(&room);
        }
        self.deps.config.forget(room);
    }

    /// Number of rooms with a live window (diagnostics / leak tests).
    pub fn tracked_rooms(&self) -> usize {
        self.windows.lock().map(|w| w.len()).unwrap_or(0)
    }

    pub async fn process(&self, stt: &SttResult) -> Outcome {
        match self.admit(stt).await {
            Admission::Rejected(o) => o,
            Admission::Admitted(a) => self.run(*a).await,
        }
    }

    /// Gates 1-4, rolling window, stage 0, cap, per-room concurrency (spec 09 section 3).
    pub async fn admit(&self, stt: &SttResult) -> Admission {
        let room = stt.meeting_id;
        if stt.is_control_marker() || stt.speaker_id == SYSTEM_SPEAKER {
            return Admission::Rejected(Outcome::Ignored);
        }
        let Some(info) = self.deps.rooms.room_info(room).await else {
            return Admission::Rejected(fin(Outcome::RoomInactive));
        };
        let workspace = info.workspace_id;
        let cfg = self.deps.config.config_for(workspace, room).await;

        // Gate 1: deploy switch.
        if !cfg.enabled {
            return Admission::Rejected(fin(Outcome::Disabled));
        }
        // Gate 2: room active.
        if !info.active {
            return Admission::Rejected(fin(Outcome::RoomInactive));
        }
        // Gate 3: platform / workspace flag.
        if !info.ai_suggest_enabled {
            return Admission::Rejected(fin(Outcome::FlagOff));
        }
        // Gate 4: host-paused transcript is skipped BEFORE the window.
        if self
            .deps
            .signals
            .transcript_paused(room, &stt.segment_id)
            .await
        {
            return Admission::Rejected(fin(Outcome::TranscriptPaused));
        }

        let text = stt.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Admission::Rejected(fin(Outcome::Stage0Rejected("empty")));
        }
        // Every segment that passes gate 4 enters the window, even when stage 0 rejects it.
        let now_ms = self.deps.clock.now().timestamp_millis();
        let turn = Turn {
            segment_id: stt.segment_id.clone(),
            speaker_id: stt.speaker_id.clone(),
            tokens: estimate_tokens(&text, cfg.window.chars_per_token),
            text,
            language: stt.language.clone(),
            at_ms: now_ms,
            role: Role::from_channel(stt.channel.as_deref()),
        };
        let context = {
            let mut windows = match self.windows.lock() {
                Ok(w) => w,
                Err(e) => e.into_inner(),
            };
            let w = windows.entry(room).or_default();
            w.push(turn.clone(), &cfg, now_ms);
            w.context()
        };

        // Stage 0.
        if let Stage0::Reject(why) = stage0::check(&cfg, &turn.text, &stt.language, stt.confidence)
        {
            return Admission::Rejected(fin(Outcome::Stage0Rejected(why)));
        }
        let answer_candidate = turn.role == Role::Other
            && stage0::asks_for_response(&turn.text, &cfg.heuristics_for(&stt.language));
        // Gate 6: cap zero disables.
        if cfg.max_per_meeting == 0 {
            return Admission::Rejected(fin(Outcome::CapZero));
        }
        // Bounded concurrency: one pipeline per room.
        let acquired = match self.inflight.lock() {
            Ok(mut s) => s.insert(room),
            Err(_) => false,
        };
        if !acquired {
            return Admission::Rejected(fin(Outcome::Busy));
        }
        Admission::Admitted(Box::new(Admitted {
            room,
            workspace,
            cfg,
            stt: stt.clone(),
            turn,
            context,
            answer_candidate,
            _guard: InflightGuard {
                set: self.inflight.clone(),
                room,
            },
        }))
    }

    /// `bypass_cooldown`: the hint about to be claimed is latency-critical.
    fn slot_params(a: &Admitted, bypass_cooldown: bool) -> SlotParams {
        SlotParams {
            bypass_cooldown,
            room: a.room,
            workspace: a.workspace,
            cooldown_seconds: a.cfg.cooldown_seconds,
            max_per_room: a.cfg.max_per_meeting,
            room_ttl_seconds: a.cfg.state_ttl_seconds,
            max_per_workspace: a.cfg.budget.max_per_workspace,
            workspace_window_seconds: a.cfg.budget.workspace_window_seconds,
        }
    }

    fn denied(d: SlotDenied) -> Outcome {
        match d {
            SlotDenied::Cooldown => Outcome::CooldownActive,
            SlotDenied::RoomBudget | SlotDenied::WorkspaceBudget => Outcome::BudgetExhausted,
        }
    }

    /// Model stages: consent, cooldown probe, decide, gates, retrieval, claim, generate, hygiene,
    /// dedup, publish.
    pub async fn run(&self, a: Admitted) -> Outcome {
        let cfg = a.cfg.clone();
        let (room, workspace) = (a.room, a.workspace);
        let consent_age = Duration::from_secs(u64::from(cfg.consent.cache_seconds));

        // Gate 7: provider + external-LLM consent, before any text leaves the process.
        let decide_llm = match self
            .deps
            .llm
            .for_purpose(workspace, Purpose::SuggestionDecide)
            .await
        {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!(%room, error = %e, "no decide provider");
                record_custom(STAGE_DECIDE, "provider_unavailable");
                return fin(Outcome::ProviderUnavailable);
            }
        };
        let mut consent: Option<bool> = None;
        if self
            .blocked_by_policy(&decide_llm, workspace, room, consent_age, &mut consent)
            .await
        {
            return fin(Outcome::BlockedPolicy);
        }

        // Gate 8: cooldown / budget probe (read-only, so a doomed attempt spends no tokens).
        // A segment that asks the user something may be an `answer`, which skips the cooldown.
        let categories: Vec<ResolvedCategory> = cfg
            .enabled_categories()
            .into_iter()
            .filter(|c| c.allows_role(a.turn.role))
            .collect();
        let may_bypass = a.answer_candidate && categories.iter().any(|c| c.skip_cooldown);
        match self
            .deps
            .state
            .probe(&Self::slot_params(&a, may_bypass))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(d)) => return fin(Self::denied(d)),
            Err(e) => {
                tracing::warn!(%room, error = %e, "slot probe failed; staying silent");
                return fin(Outcome::SlotDenied);
            }
        }

        // Stage 1: decide.
        if categories.is_empty() {
            return fin(Outcome::CategoryDisabled);
        }
        let decision = match self.decide(&a, &decide_llm, &categories).await {
            Ok(d) => d,
            Err(()) => return fin(Outcome::DecideFailed),
        };
        if !decision.should_suggest {
            tracing::debug!(%room, reason = %decision.reason, "decide declined");
            return fin(Outcome::DecideDeclined);
        }
        let all = cfg.resolved_categories();
        let Some(category) = all
            .iter()
            .find(|c| c.id == decision.category || c.wire_name == decision.category)
            .cloned()
        else {
            tracing::warn!(%room, category = %decision.category, "unknown category from decide");
            return fin(Outcome::UnknownCategory);
        };
        if !category.enabled {
            return fin(Outcome::CategoryDisabled);
        }
        if !category.allows_role(a.turn.role) {
            return fin(Outcome::RoleNotAllowed);
        }
        // The probe may have skipped the cooldown for a possible `answer`; any other category
        // must still respect it (checked before spending the generate call).
        let bypass = category.skip_cooldown && a.answer_candidate;
        if may_bypass && !bypass {
            match self.deps.state.probe(&Self::slot_params(&a, false)).await {
                Ok(Ok(())) => {}
                Ok(Err(d)) => return fin(Self::denied(d)),
                Err(_) => return fin(Outcome::SlotDenied),
            }
        }
        if decision.confidence < cfg.confidence_floor(&category) {
            return fin(Outcome::BelowConfidence);
        }

        // Grounding: "fact needs docs".
        let snippets = match self.retrieve(&a, &category).await {
            Ok(s) => s,
            Err(()) => return fin(Outcome::NeedsDocuments),
        };

        // Generate provider (resolved before the claim so a failure burns no slot).
        let gen_llm = match self
            .deps
            .llm
            .for_purpose(workspace, Purpose::SuggestionGenerate)
            .await
        {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!(%room, error = %e, "no generate provider");
                record_custom(STAGE_GENERATE, "provider_unavailable");
                return fin(Outcome::ProviderUnavailable);
            }
        };
        if self
            .blocked_by_policy(&gen_llm, workspace, room, consent_age, &mut consent)
            .await
        {
            return fin(Outcome::BlockedPolicy);
        }

        // Atomic claim (cooldown + budgets).
        match self.deps.state.claim(&Self::slot_params(&a, bypass)).await {
            Ok(Ok(())) => {}
            Ok(Err(d)) => return fin(Self::denied(d)),
            Err(e) => {
                tracing::warn!(%room, error = %e, "slot claim failed; staying silent");
                return fin(Outcome::SlotDenied);
            }
        }

        // Stage 2: generate.
        let readers = self.reader_languages(&a).await;
        let generated = match self
            .generate(&a, &gen_llm, &category, &decision, &snippets, &readers)
            .await
        {
            Ok(g) => g,
            Err(()) => return fin(Outcome::GenerateFailed),
        };
        self.finish(&a, &category, &decision, generated, &snippets, &readers)
            .await
    }

    async fn blocked_by_policy(
        &self,
        llm: &ResolvedLlm,
        workspace: WorkspaceId,
        room: RoomId,
        max_age: Duration,
        cached: &mut Option<bool>,
    ) -> bool {
        if llm.boundary == DataBoundary::WorkspaceControlled {
            return false;
        }
        let allowed = match cached {
            Some(v) => *v,
            None => {
                let v = self
                    .deps
                    .consent
                    .external_llm_allowed(workspace, room, max_age)
                    .await;
                *cached = Some(v);
                v
            }
        };
        if !allowed {
            record_custom(STAGE_DECIDE, "blocked_policy");
            tracing::debug!(%room, "external LLM not allowed; staying silent");
        }
        !allowed
    }

    fn scrub(cfg: &SuggestionConfig, text: &str) -> String {
        if cfg.hygiene.scrub_input {
            hygiene::scrub_pii(text, &cfg.hygiene)
        } else {
            text.to_string()
        }
    }

    /// The perspective rule, only when the window carries speaker roles (copilot sessions).
    fn perspective(a: &Admitted) -> String {
        let has_roles = a
            .context
            .iter()
            .chain(std::iter::once(&a.turn))
            .any(|t| t.role != Role::Unknown);
        if has_roles {
            prompt::resolve(&a.cfg, "perspective_rule").text
        } else {
            String::new()
        }
    }

    fn render_transcript(a: &Admitted, with_ids: bool) -> String {
        let line = |t: &Turn, latest: bool| {
            let name = t.role.label().unwrap_or(t.speaker_id.as_str());
            let who = if with_ids {
                format!("#{} {}", t.segment_id, name)
            } else {
                name.to_string()
            };
            let text = Self::scrub(&a.cfg, &t.text)
                .replace("</transcript>", "")
                .replace("<transcript>", "");
            let mark = if latest { "   <-- LATEST" } else { "" };
            format!("[{who}] {text}{mark}")
        };
        let mut lines: Vec<String> = a.context.iter().map(|t| line(t, false)).collect();
        lines.push(line(&a.turn, true));
        format!("<transcript>\n{}\n</transcript>", lines.join("\n"))
    }

    fn request(cfg: &SuggestionConfig, workspace: WorkspaceId, spec: RequestSpec) -> Request {
        let mut req = Request::new(spec.model, vec![Message::user(spec.user)]);
        req.system = Some(spec.system);
        req.response_format = Some(spec.schema);
        req.max_output_tokens = Some(spec.max_tokens);
        req.temperature = Some(cfg.temperature);
        if cfg.prompt_cache {
            req.cache_hint = Some(CacheHint {
                stable_prefix_messages: 0,
                key: Some(format!("suggestion:{workspace}")),
            });
        }
        req.metadata
            .insert("purpose".into(), spec.purpose.as_str().into());
        req.metadata.insert("prompt_version".into(), spec.version);
        req
    }

    async fn call(
        stage: &'static str,
        llm: &ResolvedLlm,
        req: &Request,
        timeout: Duration,
    ) -> Result<Response, CallError> {
        let timer = StageTimer::start(stage);
        let result = tokio::time::timeout(timeout, llm.provider.complete(req)).await;
        timer.finish();
        match result {
            Err(_) => {
                record_stage_outcome(stage, Metric::Timeout);
                Err(CallError::Timeout)
            }
            Ok(Err(e)) => {
                record_stage_outcome(stage, metric_for(&e));
                tracing::debug!(stage, error = %e, "llm call failed");
                Err(CallError::Failed)
            }
            Ok(Ok(r)) => {
                record_stage_outcome(stage, Metric::Ok);
                Ok(r)
            }
        }
    }

    fn untrusted(cfg: &SuggestionConfig) -> String {
        prompt::resolve(cfg, "untrusted_rule").text
    }

    async fn decide(
        &self,
        a: &Admitted,
        llm: &ResolvedLlm,
        categories: &[ResolvedCategory],
    ) -> Result<Decision, ()> {
        let cfg = &a.cfg;
        let sys_t = prompt::resolve(cfg, "decide");
        let user_t = prompt::resolve(cfg, "decide_user");
        let mut vars: BTreeMap<&str, String> = BTreeMap::new();
        vars.insert("untrusted_rule", Self::untrusted(cfg));
        vars.insert("perspective_rule", Self::perspective(a));
        vars.insert(
            "answer_examples",
            if !Self::perspective(a).is_empty() && categories.iter().any(|c| c.id == "answer") {
                prompt::resolve(cfg, "decide_answer_examples").text
            } else {
                String::new()
            },
        );
        vars.insert(
            "category_descriptions",
            categories
                .iter()
                .map(|c| format!("- {}", c.description))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        vars.insert(
            "category_ids",
            categories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        let system = prompt::render(&sys_t.text, &vars);
        let mut uvars: BTreeMap<&str, String> = BTreeMap::new();
        uvars.insert("transcript", Self::render_transcript(a, false));
        let user = prompt::render(&user_t.text, &uvars);

        let mut ids: Vec<Value> = categories.iter().map(|c| json!(c.id)).collect();
        ids.push(json!(""));
        let schema = JsonSchemaFormat {
            name: "suggestion_decision".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "should_suggest": {"type": "boolean"},
                    "category": {"type": "string", "enum": ids},
                    "confidence": {"type": "number"},
                    "reason": {"type": "string"}
                },
                "required": ["should_suggest", "category", "confidence", "reason"],
                "additionalProperties": false
            }),
            strict: true,
        };
        let model = llm
            .model
            .clone()
            .unwrap_or_else(|| cfg.decide_model.clone());
        let req = Self::request(
            cfg,
            a.workspace,
            RequestSpec {
                model,
                system,
                user,
                schema,
                max_tokens: cfg.decide_max_tokens,
                purpose: Purpose::SuggestionDecide,
                version: sys_t.version.clone(),
            },
        );
        let timeout = Duration::from_secs_f32(cfg.request_timeout_seconds);
        // No retries: a late hint is worthless.
        let resp = Self::call(STAGE_DECIDE, llm, &req, timeout)
            .await
            .map_err(|_| ())?;
        let tokens = resp.usage.total();
        self.meter(a, llm, &req.model, STAGE_DECIDE, &resp.usage);
        let Some(v) = extract_json(&resp.text()) else {
            tracing::debug!(room = %a.room, "decide returned no JSON");
            return Err(());
        };
        let should = v
            .get("should_suggest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let confidence = v
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0) as f32;
        let reason: String = str_field(&v, "reason").chars().take(120).collect();
        Ok(Decision {
            should_suggest: should,
            category: str_field(&v, "category").to_lowercase(),
            confidence,
            reason,
            tokens,
        })
    }

    async fn retrieve(
        &self,
        a: &Admitted,
        category: &ResolvedCategory,
    ) -> Result<Vec<GroundedSnippet>, ()> {
        if category.retrieval == RetrievalMode::Never {
            return Ok(Vec::new());
        }
        let r = &a.cfg.retrieval;
        let q = KnowledgeQuery {
            workspace_id: a.workspace,
            room_id: a.room,
            query: a.turn.text.clone(),
            top_k: r.top_k,
        };
        let fetched = tokio::time::timeout(
            Duration::from_millis(u64::from(r.timeout_ms)),
            self.deps.knowledge.retrieve(&q),
        )
        .await;
        let mut snippets = match fetched {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::debug!(room = %a.room, error = %e, "retrieval failed");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!(room = %a.room, "retrieval timed out");
                Vec::new()
            }
        };
        snippets.truncate(r.top_k as usize);
        for s in &mut snippets {
            s.text = hygiene::truncate_chars(&s.text, r.max_snippet_chars as usize);
        }
        if snippets.is_empty() && category.retrieval == RetrievalMode::Required {
            return Err(());
        }
        Ok(snippets)
    }

    /// Listener languages other than the segment's, normalised, sorted, capped.
    async fn reader_languages(&self, a: &Admitted) -> Vec<String> {
        if a.cfg.language.mode != LanguageMode::Listener {
            return Vec::new();
        }
        let own = primary_language(&a.stt.language);
        let mut set: Vec<String> = self
            .deps
            .signals
            .reader_languages(a.room)
            .await
            .iter()
            .map(|c| primary_language(c))
            .filter(|c| !c.is_empty() && *c != own)
            .collect();
        set.sort();
        set.dedup();
        set.truncate(a.cfg.language.max_reader_languages as usize);
        set
    }

    async fn generate(
        &self,
        a: &Admitted,
        llm: &ResolvedLlm,
        category: &ResolvedCategory,
        decision: &Decision,
        snippets: &[GroundedSnippet],
        readers: &[String],
    ) -> Result<Generated, ()> {
        let cfg = &a.cfg;
        let structured = category.kind == CategoryKind::FollowUp;
        let sys_t = prompt::resolve(cfg, "generate");
        let user_t = prompt::resolve(cfg, "generate_user");

        let out_lang = match cfg.language.mode {
            LanguageMode::Fixed => cfg.language.fixed.clone(),
            _ => a.stt.language.clone(),
        };
        let mut vars: BTreeMap<&str, String> = BTreeMap::new();
        vars.insert("untrusted_rule", Self::untrusted(cfg));
        vars.insert("perspective_rule", Self::perspective(a));
        vars.insert(
            "answer_examples",
            if category.id == "answer" {
                prompt::resolve(cfg, "generate_answer_examples").text
            } else {
                String::new()
            },
        );
        vars.insert("contract", category.contract.clone());
        vars.insert("category", category.id.clone());
        vars.insert("max_chars", cfg.max_suggestion_chars.to_string());
        vars.insert(
            "language",
            if out_lang.trim().is_empty() {
                "match the transcript".to_string()
            } else {
                cfg.language_name(&out_lang)
            },
        );
        vars.insert(
            "general_knowledge_rule",
            if category.allow_general_knowledge {
                prompt::resolve(cfg, "general_knowledge_rule").text
            } else {
                String::new()
            },
        );
        vars.insert(
            "follow_up_rule",
            if structured {
                prompt::resolve(cfg, "follow_up_rule").text
            } else {
                String::new()
            },
        );
        let mut extra = String::new();
        if !readers.is_empty() {
            extra.push_str(", \"translations\": object");
        }
        if structured {
            extra.push_str(", \"follow_up\": object");
        }
        vars.insert("extra_fields", extra);
        let mut tvars: BTreeMap<&str, String> = BTreeMap::new();
        let wanted = readers
            .iter()
            .map(|c| format!("\"{c}\" ({})", cfg.language_name(c)))
            .collect::<Vec<_>>()
            .join(", ");
        tvars.insert("wanted_languages", wanted);
        vars.insert(
            "translations_rule",
            if readers.is_empty() {
                String::new()
            } else {
                prompt::render(&prompt::resolve(cfg, "translations_rule").text, &tvars)
            },
        );
        vars.insert("reader_languages", readers.join(", "));
        let system = prompt::render(&sys_t.text, &vars);

        let mut docs = String::new();
        if !snippets.is_empty() {
            docs.push_str("\n\n<reference_sources>\n");
            for (i, s) in snippets.iter().enumerate() {
                docs.push_str(&format!(
                    "--- Source S{}: {} ---\n{}\n",
                    i + 1,
                    s.title,
                    s.text
                ));
            }
            docs.push_str("</reference_sources>");
        }
        let mut uvars: BTreeMap<&str, String> = BTreeMap::new();
        uvars.insert("category", category.id.clone());
        uvars.insert("reason", decision.reason.clone());
        uvars.insert(
            "language",
            if a.stt.language.trim().is_empty() {
                "match the transcript".to_string()
            } else {
                a.stt.language.clone()
            },
        );
        uvars.insert("segment_language", a.stt.language.clone());
        uvars.insert("transcript", Self::render_transcript(a, structured));
        uvars.insert("documents", docs);
        let user = prompt::render(&user_t.text, &uvars);

        let mut props = json!({
            "content": {"type": "string"},
            "detail": {"type": "string"},
            "source": {"type": "string"},
            "conflict": {"type": "boolean"}
        });
        if let Some(p) = props.as_object_mut() {
            if !readers.is_empty() {
                p.insert(
                    "translations".into(),
                    json!({"type": "object", "additionalProperties": {
                        "type": "object",
                        "properties": {"content": {"type": "string"}, "detail": {"type": "string"}}
                    }}),
                );
            }
            if structured {
                p.insert(
                    "follow_up".into(),
                    json!({"type": "object", "properties": {
                        "owner": {"type": ["string", "null"]},
                        "task": {"type": "string"},
                        "due": {"type": ["string", "null"]},
                        "source_segment_ids": {"type": "array", "items": {"type": "string"}}
                    }}),
                );
            }
        }
        let schema = JsonSchemaFormat {
            name: "suggestion_hint".into(),
            schema: json!({
                "type": "object",
                "properties": props,
                "required": ["content", "detail", "source", "conflict"]
            }),
            strict: false,
        };
        let multiplier = if cfg.language.scale_tokens_per_language {
            1 + readers.len() as u32 + u32::from(structured)
        } else {
            1
        };
        let model = llm
            .model
            .clone()
            .unwrap_or_else(|| cfg.generate_model.clone());
        let req = Self::request(
            cfg,
            a.workspace,
            RequestSpec {
                model,
                system,
                user,
                schema,
                max_tokens: cfg.generate_max_tokens.saturating_mul(multiplier),
                purpose: Purpose::SuggestionGenerate,
                version: sys_t.version.clone(),
            },
        );
        let timeout = Duration::from_secs_f32(cfg.request_timeout_seconds);
        let resp = Self::call(STAGE_GENERATE, llm, &req, timeout)
            .await
            .map_err(|_| ())?;
        let tokens = resp.usage.total();
        self.meter(a, llm, &req.model, STAGE_GENERATE, &resp.usage);
        let Some(v) = extract_json(&resp.text()) else {
            return Err(());
        };
        let mut translations = BTreeMap::new();
        if let Some(obj) = v.get("translations").and_then(Value::as_object) {
            for code in readers {
                if let Some(e) = obj.get(code) {
                    let c = str_field(e, "content");
                    if !c.is_empty() {
                        translations.insert(code.clone(), (c, str_field(e, "detail")));
                    }
                }
            }
        }
        Ok(Generated {
            content: str_field(&v, "content"),
            detail: str_field(&v, "detail"),
            source: str_field(&v, "source"),
            conflict: v.get("conflict").and_then(Value::as_bool).unwrap_or(false),
            translations,
            follow_up: v.get("follow_up").filter(|f| f.is_object()).cloned(),
            tokens,
        })
    }

    fn sources(generated_source: &str, snippets: &[GroundedSnippet]) -> Vec<SourceRef> {
        let wanted = generated_source.trim().to_lowercase();
        if wanted.is_empty() {
            return Vec::new();
        }
        snippets
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                wanted == format!("s{}", i + 1) || wanted == s.title.trim().to_lowercase()
            })
            .map(|(i, s)| SourceRef {
                marker: format!("S{}", i + 1),
                kind: s.kind.clone(),
                title: s.title.clone(),
                chunk_id: Some(s.chunk_id.clone()),
            })
            .collect()
    }

    fn build_follow_up(raw: &Value, a: &Admitted) -> Option<FollowUp> {
        let fc = &a.cfg.follow_up;
        let scrub = |s: &str, max: u32| {
            let cleaned = hygiene::clean_text(s, max as usize);
            if a.cfg.hygiene.redact_pii {
                hygiene::scrub_pii(&cleaned, &a.cfg.hygiene)
            } else {
                cleaned
            }
        };
        let task = scrub(&str_field(raw, "task"), fc.task_max_chars);
        if task.is_empty() {
            return None;
        }
        let corpus = a
            .context
            .iter()
            .chain(std::iter::once(&a.turn))
            .map(|t| t.text.to_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        let speakers: Vec<&str> = a
            .context
            .iter()
            .chain(std::iter::once(&a.turn))
            .map(|t| t.speaker_id.as_str())
            .collect();
        let verified = |s: &str, allow_speaker: bool| {
            !fc.verify_in_transcript
                || corpus.contains(&s.to_lowercase())
                || (allow_speaker && speakers.contains(&s))
        };
        let opt = |key: &str, max: u32, allow_speaker: bool| -> Option<String> {
            let s = hygiene::clean_text(&str_field(raw, key), max as usize);
            (!s.is_empty() && verified(&s, allow_speaker)).then_some(s)
        };
        let owner = opt("owner", fc.owner_max_chars, true);
        let due = opt("due", fc.due_max_chars, false);
        let known: Vec<&str> = a
            .context
            .iter()
            .chain(std::iter::once(&a.turn))
            .map(|t| t.segment_id.as_str())
            .collect();
        let mut ids: Vec<String> = Vec::new();
        if let Some(arr) = raw.get("source_segment_ids").and_then(Value::as_array) {
            for id in arr.iter().filter_map(Value::as_str) {
                let id = id.trim().trim_start_matches('#');
                if known.contains(&id) && !ids.iter().any(|x| x == id) {
                    ids.push(id.to_string());
                }
            }
        }
        if ids.is_empty() {
            ids.push(a.turn.segment_id.clone());
        }
        Some(FollowUp {
            owner,
            task,
            due,
            source_segment_ids: ids,
        })
    }

    /// Output hygiene, dedup and publishing.
    async fn finish(
        &self,
        a: &Admitted,
        category: &ResolvedCategory,
        decision: &Decision,
        g: Generated,
        snippets: &[GroundedSnippet],
        readers: &[String],
    ) -> Outcome {
        let cfg = &a.cfg;
        if g.content.trim().is_empty() {
            return fin(Outcome::GenerateEmpty);
        }
        if hygiene::contains_blocked(&g.content, &cfg.hygiene)
            || hygiene::contains_blocked(&g.detail, &cfg.hygiene)
        {
            tracing::warn!(room = %a.room, "generate output contained a blocked phrase");
            return fin(Outcome::HygieneDropped);
        }
        let clean = |s: &str, max: u32| {
            let c = hygiene::clean_text(s, max as usize);
            if cfg.hygiene.redact_pii {
                hygiene::scrub_pii(&c, &cfg.hygiene)
            } else {
                c
            }
        };
        let content = clean(&g.content, cfg.max_suggestion_chars);
        if content.chars().count() < cfg.hygiene.min_content_chars as usize {
            return fin(Outcome::GenerateEmpty);
        }
        let detail = clean(&g.detail, cfg.max_detail_chars);

        if cfg.dedup.window_seconds > 0 {
            let hash = hygiene::dedup_hash(&category.id, &content);
            match self
                .deps
                .state
                .first_seen(a.room, &hash, cfg.dedup.window_seconds)
                .await
            {
                Ok(true) => {}
                Ok(false) => return fin(Outcome::Duplicate),
                Err(e) => tracing::warn!(room = %a.room, error = %e, "dedup unavailable"),
            }
        }

        let follow_up = if category.kind == CategoryKind::FollowUp {
            g.follow_up
                .as_ref()
                .and_then(|raw| Self::build_follow_up(raw, a))
        } else {
            None
        };
        let language = match cfg.language.mode {
            LanguageMode::Fixed => cfg.language.fixed.clone(),
            _ => a.stt.language.clone(),
        };
        let now_ms = self.deps.clock.now().timestamp_millis();
        let prompt_version = prompt::resolve(cfg, "generate").version;
        let result = SuggestionResult {
            suggestion_id: uuid::Uuid::now_v7().to_string(),
            meeting_id: a.room,
            workspace_id: a.workspace,
            segment_id: a.stt.segment_id.clone(),
            kind: "suggestion".into(),
            category: category.wire_name.clone(),
            severity: category.severity.clone(),
            content,
            detail,
            confidence: decision.confidence,
            language,
            token_count: decision.tokens.saturating_add(g.tokens),
            sources: Self::sources(&g.source, snippets),
            timestamp_ms: now_ms,
            ttl_seconds: category.ttl_seconds,
            conflict: g.conflict,
            follow_up,
            actions: category.actions.clone(),
            prompt_version,
        };

        let mut copies = vec![result.clone()];
        for code in readers {
            if let Some((c, d)) = g.translations.get(code) {
                let content = clean(c, cfg.max_suggestion_chars);
                if content.is_empty() || hygiene::contains_blocked(&content, &cfg.hygiene) {
                    continue;
                }
                let mut copy = result.clone();
                copy.content = content;
                copy.detail = clean(d, cfg.max_detail_chars);
                copy.language = code.clone();
                copy.token_count = 0;
                copies.push(copy);
            }
        }

        let mut conn = self.deps.redis.clone();
        let maxlen = cfg.delivery.stream_maxlen as usize;
        let per_meeting = format!("{AI_ASSISTANT_RESULTS}:{}", a.room);
        for copy in &copies {
            let fields = copy.to_fields();
            for stream in [AI_ASSISTANT_RESULTS, per_meeting.as_str()] {
                if let Err(e) = xadd(&mut conn, stream, &fields, maxlen).await {
                    tracing::warn!(room = %a.room, stream, error = %e, "publish failed");
                    return fin(Outcome::PublishFailed);
                }
            }
        }
        if let Some(fu) = &result.follow_up {
            let event = FollowUpEvent {
                suggestion_id: result.suggestion_id.clone(),
                meeting_id: a.room,
                workspace_id: a.workspace,
                segment_id: result.segment_id.clone(),
                category: category.id.clone(),
                follow_up: fu.clone(),
                confidence: result.confidence,
                language: result.language.clone(),
                timestamp_ms: now_ms,
            };
            if let Err(e) = warptalk_bus::xadd_json(
                &mut conn,
                ASSISTANT_FOLLOWUPS,
                &event,
                cfg.delivery.followup_stream_maxlen as usize,
            )
            .await
            {
                tracing::warn!(room = %a.room, error = %e, "follow-up publish failed");
            }
        }
        tracing::info!(
            room = %a.room,
            segment = %a.stt.segment_id,
            category = %category.id,
            confidence = decision.confidence,
            tokens = result.token_count,
            "suggestion published"
        );
        fin(Outcome::Published)
    }
}
