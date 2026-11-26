//! Replays meetings through the real `SuggestionEngine` and `SummaryService` and scores them.
//!
//! Suggestions: the engine is driven line by line (`admit` + model stages, exactly what the
//! `SuggestionWorker` does per segment) with in-memory ports for rooms, consent, signals and
//! knowledge. Its slot/dedup state and result stream live in Redis because the engine owns
//! those; keys are namespaced and removed after each meeting.
//!
//! Summary: the real `SummaryService` (`on_meeting_end` then `run`) with in-memory transcript
//! and room sources; the job row lives in Postgres and is deleted afterwards.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use redis::streams::StreamRangeReply;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_assistant::suggestion::ports::RetrieveError;
use warptalk_assistant::suggestion::{
    AI_ASSISTANT_RESULTS, AllowAllConsent, DataBoundary, EngineDeps, GroundedSnippet,
    KnowledgeQuery, KnowledgeRetriever, LlmResolver, NoSignals, Purpose, ResolveError, ResolvedLlm,
    RoomDirectory, RoomInfo, StaticConfig, SuggestionConfig, SuggestionEngine, SuggestionResult,
    SuggestionState,
};
use warptalk_assistant::summary::{
    AllowAllExternalLlm, NoBus, NoFollowUps, PgSummaryRepository, RoomMeta, RoomMetaSource,
    RunOutcome, SummaryDeps, SummaryService, TranscriptData, TranscriptSegment, TranscriptSource,
};
use warptalk_contracts::events::SttResult;
use warptalk_kernel::{AppResult, Clock, RoomId, SystemClock, WorkspaceId};
use warptalk_llm::LlmProvider;
use warptalk_settings::{SettingsStore, Validate};

use crate::cost::Price;
use crate::dataset::{Meeting, Snippet};
use crate::judge::{Judge, JudgeStats};
use crate::providers::ProviderSpec;
use crate::recorder::{CallRecord, RecordingProvider, latency_stats, usage_totals};
use crate::report::{
    CategoryReport, JudgeInfo, MeetingSuggestions, MeetingSummary, Report, SCHEMA_VERSION,
    SuggestionsReport, SummaryReport,
};
use crate::scoring::{
    CategoryCounts, Counts, InjectionMetrics, OutAction, Published, SummaryCounts, SummaryOutput,
    injection_hit_summary, injection_hits_suggestions, is_attack, score_suggestions, score_summary,
};
use crate::textmatch;

pub const DEFAULT_DATABASE_URL: &str = "postgres://postgres@localhost:5432/warptalk";
pub const DEFAULT_REDIS_URL: &str = "redis://localhost:6379";
pub(crate) const EPOCH_MS_PUB: i64 = EPOCH_MS;
const STATE_PREFIX: &str = "evalsuggest";
/// Fixed replay epoch so window ages are reproducible.
const EPOCH_MS: i64 = 1_800_000_000_000;

// ------------------------------------------------------------------ ports

struct ReplayClock(AtomicI64);

impl ReplayClock {
    fn set(&self, t_ms: i64) {
        self.0.store(EPOCH_MS + t_ms, Ordering::SeqCst);
    }
}

impl Clock for ReplayClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_millis(self.0.load(Ordering::SeqCst))
            .unwrap_or_else(Utc::now)
    }
}

pub(crate) struct FixedResolver {
    pub(crate) provider: Arc<dyn LlmProvider>,
    pub(crate) model: String,
}

#[async_trait]
impl LlmResolver for FixedResolver {
    async fn for_purpose(&self, _: WorkspaceId, _: Purpose) -> Result<ResolvedLlm, ResolveError> {
        Ok(ResolvedLlm {
            provider: self.provider.clone(),
            model: Some(self.model.clone()),
            boundary: DataBoundary::External,
            key_source: Default::default(),
        })
    }
}

pub(crate) struct OneRoom(pub(crate) WorkspaceId);

#[async_trait]
impl RoomDirectory for OneRoom {
    async fn room_info(&self, _: RoomId) -> Option<RoomInfo> {
        Some(RoomInfo {
            workspace_id: self.0,
            active: true,
            ai_suggest_enabled: true,
        })
    }
}

/// Toy retriever: ranks the meeting's snippets by word overlap with the query.
pub(crate) struct SnippetRetriever(pub(crate) Vec<Snippet>);

#[async_trait]
impl KnowledgeRetriever for SnippetRetriever {
    async fn retrieve(&self, q: &KnowledgeQuery) -> Result<Vec<GroundedSnippet>, RetrieveError> {
        let query = textmatch::words(&q.query);
        let mut scored: Vec<(usize, usize)> = self
            .0
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let body = textmatch::words(&format!("{} {}", s.title, s.text));
                (i, query.iter().filter(|w| body.contains(w)).count())
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(scored
            .into_iter()
            .take(q.top_k as usize)
            .map(|(i, n)| GroundedSnippet {
                chunk_id: format!("snippet-{i}"),
                title: self.0[i].title.clone(),
                text: self.0[i].text.clone(),
                score: n as f32,
                kind: self.0[i].kind.clone(),
            })
            .collect())
    }
}

struct MemTranscript {
    workspace: WorkspaceId,
    segments: Vec<TranscriptSegment>,
}

#[async_trait]
impl TranscriptSource for MemTranscript {
    async fn load(&self, _: RoomId, limit: u32) -> AppResult<Option<TranscriptData>> {
        let mut segments = self.segments.clone();
        if limit > 0 {
            segments.truncate(limit as usize);
        }
        Ok(Some(TranscriptData {
            workspace_id: self.workspace,
            version: 1,
            finalized: true,
            segments,
            total_segments: self.segments.len() as u32,
        }))
    }
}

struct MemRoom(RoomMeta);

#[async_trait]
impl RoomMetaSource for MemRoom {
    async fn room(&self, _: RoomId) -> AppResult<Option<RoomMeta>> {
        Ok(Some(self.0.clone()))
    }
}

// ------------------------------------------------------------------ infrastructure

/// Connections the suites need.
pub struct Infra {
    pub redis: Option<ConnectionManager>,
    pub pool: Option<PgPool>,
    pub store: Option<SettingsStore>,
}

impl Infra {
    pub async fn connect(
        need_redis: bool,
        need_pg: bool,
        redis_url: &str,
        database_url: &str,
    ) -> Result<Self> {
        let redis = if need_redis {
            Some(
                warptalk_bus::connect(redis_url)
                    .await
                    .map_err(|e| anyhow!("cannot reach Redis (set REDIS_URL): {e}"))?,
            )
        } else {
            None
        };
        let (pool, store) = if need_pg {
            let pool = warptalk_db::connect(database_url, 4)
                .await
                .map_err(|e| anyhow!("cannot reach Postgres (set DATABASE_URL): {e}"))?;
            warptalk_db::run_migrations(
                &pool,
                &[
                    warptalk_settings::MIGRATIONS,
                    warptalk_assistant::MIGRATIONS,
                ],
            )
            .await
            .map_err(|e| anyhow!("migrations failed: {e}"))?;
            let store = SettingsStore::new(pool.clone(), None);
            warptalk_settings::register_defaults(&store);
            warptalk_assistant::summary::register_config(&store);
            (Some(pool), Some(store))
        } else {
            (None, None)
        };
        Ok(Self { redis, pool, store })
    }
}

// ------------------------------------------------------------------ suggestions

/// Defaults of the eval: replay is much faster than real time, so cooldown is off and the
/// cap is high; model calls may be slower than the live 8 s budget.
pub fn suggestion_config(overrides: Option<&Value>) -> Result<SuggestionConfig> {
    let mut doc = json!({
        "enabled": true,
        "cooldown_seconds": 0,
        "max_per_meeting": 200,
        "request_timeout_seconds": 30.0
    });
    if let (Some(Value::Object(extra)), Some(base)) = (overrides, doc.as_object_mut()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    let cfg: SuggestionConfig =
        serde_json::from_value(doc).context("invalid suggestion configuration")?;
    cfg.validate()
        .map_err(|e| anyhow!("invalid suggestion configuration: {e}"))?;
    Ok(cfg)
}

/// The engine's input for one dataset line. The line's `channel` is carried by the dataset;
/// this is the single place that maps it onto the engine's segment type.
pub fn stt_for_line(m: &Meeting, i: usize, room: RoomId) -> SttResult {
    let l = &m.lines[i];
    SttResult {
        segment_id: format!("seg-{i:03}"),
        meeting_id: room,
        speaker_id: l.speaker.clone(),
        text: l.text.clone(),
        language: m.language.clone(),
        start_ms: l.t_ms,
        end_ms: l.t_ms + 1000,
        anchor_ms: None,
        confidence: None,
        chunk_index: None,
        is_final_chunk: false,
        timestamp_ms: None,
        clean_flags: None,
        far_speaker_name: None,
        far_speaker_source: None,
        far_speaker_confidence: None,
        channel: match l.channel {
            crate::dataset::Channel::Me => Some("me".into()),
            crate::dataset::Channel::Others => Some("others".into()),
            crate::dataset::Channel::Unknown => None,
        },
    }
}

pub struct SuggestionRun {
    pub published: Vec<Published>,
    /// (line, result) as the engine published it.
    pub results: Vec<(usize, SuggestionResult)>,
    pub outcomes: BTreeMap<String, u64>,
    pub records: Vec<CallRecord>,
}

fn published_text(r: &SuggestionResult) -> String {
    let mut t = format!("{}\n{}", r.content, r.detail);
    if let Some(f) = &r.follow_up {
        t.push_str(&format!(
            "\n{} {} {}",
            f.task,
            f.owner.as_deref().unwrap_or(""),
            f.due.as_deref().unwrap_or("")
        ));
    }
    t
}

async fn read_results(redis: &ConnectionManager, key: &str) -> Result<Vec<SuggestionResult>> {
    let mut conn = redis.clone();
    let reply: StreamRangeReply = redis::cmd("XRANGE")
        .arg(key)
        .arg("-")
        .arg("+")
        .query_async(&mut conn)
        .await
        .context("reading the suggestion result stream")?;
    Ok(reply
        .ids
        .iter()
        .filter_map(|id| id.map.get("data"))
        .filter_map(|v| redis::from_redis_value::<String>(v).ok())
        .filter_map(|s| serde_json::from_str::<SuggestionResult>(&s).ok())
        .collect())
}

pub async fn replay_suggestions(
    redis: &ConnectionManager,
    spec: &ProviderSpec,
    model: &str,
    cfg: &SuggestionConfig,
    m: &Meeting,
) -> Result<SuggestionRun> {
    let (ws, room) = (WorkspaceId::new(), RoomId::new());
    let rec = Arc::new(RecordingProvider::new(spec.for_meeting(m), spec.is_fake()));
    let clock = Arc::new(ReplayClock(AtomicI64::new(EPOCH_MS)));
    let engine = SuggestionEngine::new(EngineDeps {
        rooms: Arc::new(OneRoom(ws)),
        signals: Arc::new(NoSignals),
        consent: Arc::new(AllowAllConsent),
        llm: Arc::new(FixedResolver {
            provider: rec.clone(),
            model: model.to_owned(),
        }),
        knowledge: Arc::new(SnippetRetriever(m.snippets.clone())),
        config: Arc::new(StaticConfig::new(cfg.clone())),
        clock: clock.clone(),
        state: SuggestionState::with_prefix(redis.clone(), STATE_PREFIX),
        redis: redis.clone(),
    });

    let key = format!("{AI_ASSISTANT_RESULTS}:{room}");
    let mut published = Vec::new();
    let mut results = Vec::new();
    let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
    let mut seen = 0usize;
    for i in 0..m.lines.len() {
        clock.set(m.lines[i].t_ms);
        let outcome = engine.process(&stt_for_line(m, i, room)).await;
        *outcomes.entry(outcome.label().to_owned()).or_default() += 1;
        if outcome.is_published() {
            let all = read_results(redis, &key).await?;
            for r in all.into_iter().skip(seen) {
                published.push(Published {
                    line: i,
                    category: r.category.clone(),
                    text: published_text(&r),
                });
                results.push((i, r));
                seen += 1;
            }
        }
    }
    engine.forget_room(room);
    let mut conn = redis.clone();
    let _: Result<i64, _> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
    Ok(SuggestionRun {
        published,
        results,
        outcomes,
        records: rec.records(),
    })
}

// ------------------------------------------------------------------ summary

pub struct SummaryRun {
    pub output: Option<SummaryOutput>,
    pub error: Option<String>,
    pub records: Vec<CallRecord>,
}

pub async fn run_summary(
    infra: &Infra,
    spec: &ProviderSpec,
    model: &str,
    m: &Meeting,
    overrides: Option<&Value>,
) -> Result<SummaryRun> {
    let (Some(pool), Some(store)) = (&infra.pool, &infra.store) else {
        bail!("summary suite needs Postgres");
    };
    let (ws, room) = (WorkspaceId::new(), RoomId::new());
    if let Some(doc) = overrides {
        store
            .put(
                warptalk_settings::Scope::Workspace(ws),
                warptalk_assistant::summary::NS_SUMMARY,
                doc.clone(),
                None,
            )
            .await
            .map_err(|e| anyhow!("invalid summary configuration: {e}"))?;
    }
    let rec = Arc::new(RecordingProvider::new(spec.for_meeting(m), spec.is_fake()));
    let segments = m
        .lines
        .iter()
        .map(|l| TranscriptSegment {
            id: Uuid::now_v7(),
            speaker: l.speaker.clone(),
            text: l.text.clone(),
            language: m.language.clone(),
            start_ms: l.t_ms,
        })
        .collect();
    let service = SummaryService::new(SummaryDeps {
        repo: PgSummaryRepository::new(pool.clone()),
        transcripts: Arc::new(MemTranscript {
            workspace: ws,
            segments,
        }),
        rooms: Arc::new(MemRoom(RoomMeta {
            workspace_id: ws,
            title: m.title.clone(),
            source_language: m.language.clone(),
            target_languages: Vec::new(),
            participants: m.participants.clone(),
        })),
        followups: Arc::new(NoFollowUps),
        llm: Arc::new(FixedResolver {
            provider: rec.clone(),
            model: model.to_owned(),
        }),
        policy: Arc::new(AllowAllExternalLlm),
        settings: store.clone(),
        bus: Arc::new(NoBus),
        clock: Arc::new(SystemClock),
    });

    let result = async {
        let job = service
            .on_meeting_end(room)
            .await
            .map_err(|e| anyhow!("enqueue failed: {e}"))?
            .ok_or_else(|| anyhow!("summary is disabled by configuration"))?;
        let outcome = service.run(job.id).await;
        let row = service
            .repo()
            .get(job.id)
            .await
            .map_err(|e| anyhow!("reading the job failed: {e}"))?
            .ok_or_else(|| anyhow!("job vanished"))?;
        Ok::<_, anyhow::Error>((outcome, row))
    }
    .await;

    // Remove the rows this run created.
    let _ = sqlx::query("DELETE FROM assistant.action_items WHERE room_id = $1")
        .bind(room.as_uuid())
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM assistant.meeting_summaries WHERE room_id = $1")
        .bind(room.as_uuid())
        .execute(pool)
        .await;

    let (outcome, row) = result?;
    let (output, error) = match (outcome, row.content) {
        (RunOutcome::Completed, Some(c)) => (
            Some(SummaryOutput {
                summary_short: c.summary_short,
                summary_detailed: c.summary_detailed,
                decisions: c.decisions,
                action_items: c
                    .action_items
                    .into_iter()
                    .map(|a| OutAction {
                        task: a.task,
                        owner: a.owner,
                        due: a.due,
                    })
                    .collect(),
                open_questions: c.open_questions,
            }),
            None,
        ),
        (o, _) => (
            None,
            Some(format!(
                "{o:?}: {}",
                row.error_code.unwrap_or_else(|| "no content".into())
            )),
        ),
    };
    Ok(SummaryRun {
        output,
        error,
        records: rec.records(),
    })
}

// ------------------------------------------------------------------ orchestration

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    Suggestions,
    Summary,
    All,
}

impl Suite {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "suggestions" => Self::Suggestions,
            "summary" => Self::Summary,
            "all" => Self::All,
            o => bail!("unknown suite `{o}` (suggestions, summary, all)"),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Suggestions => "suggestions",
            Self::Summary => "summary",
            Self::All => "all",
        }
    }
    fn suggestions(self) -> bool {
        matches!(self, Self::Suggestions | Self::All)
    }
    fn summary(self) -> bool {
        matches!(self, Self::Summary | Self::All)
    }
}

pub struct JudgeSetup {
    pub provider_name: String,
    pub model: String,
    pub spec: ProviderSpec,
}

pub struct RunOptions {
    pub suite: Suite,
    pub provider: ProviderSpec,
    pub model: String,
    pub judge: Option<JudgeSetup>,
    pub meetings: Vec<Meeting>,
    pub tolerance: usize,
    pub suggestion_overrides: Option<Value>,
    pub summary_overrides: Option<Value>,
    pub price: Option<Price>,
    pub redis_url: String,
    pub database_url: String,
}

fn by_purpose(records: &[CallRecord], purpose: &str) -> Vec<CallRecord> {
    records
        .iter()
        .filter(|r| r.purpose == purpose)
        .cloned()
        .collect()
}

pub async fn run(opts: &RunOptions) -> Result<Report> {
    let infra = Infra::connect(
        opts.suite.suggestions(),
        opts.suite.summary(),
        &opts.redis_url,
        &opts.database_url,
    )
    .await?;
    let judge = opts.judge.as_ref().map(|j| {
        Judge::new(
            Arc::new(RecordingProvider::new(j.spec.for_judge(), false)),
            j.model.clone(),
        )
    });
    let price = opts.price.or_else(|| crate::cost::lookup(&opts.model));

    let suggestions = if opts.suite.suggestions() {
        let redis = infra.redis.as_ref().ok_or_else(|| anyhow!("no Redis"))?;
        let cfg = suggestion_config(opts.suggestion_overrides.as_ref())?;
        Some(run_suggestions(redis, opts, &cfg, judge.as_ref(), price).await?)
    } else {
        None
    };
    let summary = if opts.suite.summary() {
        Some(run_summaries(&infra, opts, judge.as_ref(), price).await?)
    } else {
        None
    };
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        created_at: Utc::now().to_rfc3339(),
        suite: opts.suite.as_str().into(),
        provider: opts.provider.name().into(),
        model: opts.model.clone(),
        judge: opts.judge.as_ref().map(|j| JudgeInfo {
            provider: j.provider_name.clone(),
            model: j.model.clone(),
        }),
        line_tolerance: opts.tolerance,
        latency_simulated: opts.provider.is_fake(),
        injection: injection_metrics(&opts.meetings, suggestions.as_ref(), summary.as_ref()),
        suggestions,
        summary,
    })
}

/// Aggregate the injection check; `None` when no attack meeting was run.
fn injection_metrics(
    meetings: &[Meeting],
    suggestions: Option<&SuggestionsReport>,
    summary: Option<&SummaryReport>,
) -> Option<InjectionMetrics> {
    let attacks: Vec<&Meeting> = meetings.iter().filter(|m| is_attack(m)).collect();
    if attacks.is_empty() {
        return None;
    }
    let (mut outputs, mut successes) = (0u64, 0u64);
    for m in attacks.iter() {
        if let Some(ms) = suggestions.and_then(|s| s.meetings.iter().find(|x| x.id == m.id)) {
            outputs += ms.published.len() as u64;
            successes += ms.injection_hits.len() as u64;
        }
        if let Some(ms) = summary.and_then(|s| s.meetings.iter().find(|x| x.id == m.id))
            && ms.error.is_none()
        {
            outputs += 1;
            successes += u64::from(ms.injection_hit.is_some());
        }
    }
    Some(InjectionMetrics::from_counts(
        attacks.len() as u64,
        outputs,
        successes,
    ))
}

async fn run_suggestions(
    redis: &ConnectionManager,
    opts: &RunOptions,
    cfg: &SuggestionConfig,
    judge: Option<&Judge>,
    price: Option<Price>,
) -> Result<SuggestionsReport> {
    let mut counts = Counts::default();
    let mut cats: BTreeMap<String, CategoryCounts> = BTreeMap::new();
    let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
    let mut records = Vec::new();
    let mut meetings = Vec::new();
    let mut judged: Vec<Option<u8>> = Vec::new();

    for m in &opts.meetings {
        eprintln!("suggestions: {}", m.id);
        let run = replay_suggestions(redis, &opts.provider, &opts.model, cfg, m).await?;
        let score = score_suggestions(m, &run.published, opts.tolerance);
        counts.add(&score.counts);
        for (k, c) in &score.by_category {
            cats.entry(k.clone()).or_default().add(c);
        }
        for (k, n) in &run.outcomes {
            *outcomes.entry(k.clone()).or_default() += n;
        }
        if let Some(j) = judge {
            for (line, r) in &run.results {
                judged.push(
                    j.suggestion(m, *line, &r.category, &published_text(r))
                        .await,
                );
            }
        }
        records.extend(run.records);
        meetings.push(MeetingSuggestions {
            id: m.id.clone(),
            metrics: score.counts.metrics(),
            expected: score.expected,
            hallucinations: score.hallucinations,
            injection_hits: injection_hits_suggestions(m, &run.published, &score.forbidden_hits),
            forbidden_hits: score.forbidden_hits,
            published: run.published,
            outcomes: run.outcomes,
        });
    }

    let mut latency = BTreeMap::new();
    for purpose in ["suggestion_decide", "suggestion_generate"] {
        let recs = by_purpose(&records, purpose);
        latency.insert(
            purpose.to_owned(),
            latency_stats(&recs.iter().collect::<Vec<_>>()),
        );
    }
    Ok(SuggestionsReport {
        metrics: counts.metrics(),
        by_category: cats
            .iter()
            .map(|(k, c)| (k.clone(), CategoryReport::from(c)))
            .collect(),
        latency,
        usage: usage_totals(&records, price),
        judge: judge.map(|_| JudgeStats::from_scores(&judged)),
        outcomes,
        meetings,
    })
}

async fn run_summaries(
    infra: &Infra,
    opts: &RunOptions,
    judge: Option<&Judge>,
    price: Option<Price>,
) -> Result<SummaryReport> {
    let mut counts = SummaryCounts::default();
    let mut records = Vec::new();
    let mut meetings = Vec::new();
    let mut judged: Vec<Option<u8>> = Vec::new();

    for m in &opts.meetings {
        eprintln!("summary: {}", m.id);
        let run = run_summary(
            infra,
            &opts.provider,
            &opts.model,
            m,
            opts.summary_overrides.as_ref(),
        )
        .await?;
        let score = score_summary(&m.expectations.summary, run.output.as_ref());
        counts.add(&score.counts);
        let judge_score = match (judge, &run.output) {
            (Some(j), Some(o)) => {
                let s = j.summary(m, &o.full_text()).await;
                judged.push(s);
                s
            }
            _ => None,
        };
        records.extend(run.records);
        meetings.push(MeetingSummary {
            id: m.id.clone(),
            metrics: score.counts.metrics(),
            error: run.error,
            missed_decisions: score.missed_decisions,
            missed_actions: score.missed_actions,
            wrong_owner: score.wrong_owner,
            wrong_deadline: score.wrong_deadline,
            traps_hit: score.traps_hit,
            injection_hit: run
                .output
                .as_ref()
                .and_then(|o| injection_hit_summary(m, o)),
            judge_score,
        });
    }
    let recs = by_purpose(&records, "summary");
    Ok(SummaryReport {
        metrics: counts.metrics(),
        latency: latency_stats(&recs.iter().collect::<Vec<_>>()),
        usage: usage_totals(&records, price),
        judge: judge.map(|_| JudgeStats::from_scores(&judged)),
        meetings,
    })
}
