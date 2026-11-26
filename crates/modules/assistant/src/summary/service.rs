//! `SummaryService`: enqueue, run (map-reduce over windows), validate, persist.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Duration as ChronoDuration;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, UserId, WorkspaceId};
use warptalk_llm::{JsonSchemaFormat, LlmError, LlmProvider, Message, Request};
use warptalk_settings::{ScopeChain, SettingsStore};
use warptalk_telemetry::{Outcome, StageTimer, record_stage_outcome};

use super::config::{NS_SUMMARY, SummaryConfig};
use super::model::{
    RawOutput, SummaryContent, SummaryJob, SummaryMeta, SummaryRequest, SummaryResultEvent,
    SummaryStatus, Trigger, codes, output_schema,
};
use super::ports::{
    ExternalLlmGate, FollowUpSource, LiveFollowUp, RoomMetaSource, SummaryBus, TranscriptData,
    TranscriptSource,
};
use super::prompt;
use super::prompt::Speakers;
use super::repo::{Enqueued, PgSummaryRepository};
use crate::memory::{MeetingMemory, PgMeetingMemory, PriorContext, PriorQuery};
use crate::safety::{self, Fence};
use crate::suggestion::{DataBoundary, LlmResolver, Purpose, ResolveError};
use crate::usage::{SharedUsageSink, UsageEvent};

const STAGE: &str = "meeting_summary";
const TOKENS_METRIC: &str = "warptalk_summary_tokens_total";

pub struct SummaryDeps {
    pub repo: PgSummaryRepository,
    pub transcripts: Arc<dyn TranscriptSource>,
    pub rooms: Arc<dyn RoomMetaSource>,
    pub followups: Arc<dyn FollowUpSource>,
    pub llm: Arc<dyn LlmResolver>,
    /// Workspace `externalLlmAllowed` policy, applied to `External` providers.
    pub policy: Arc<dyn ExternalLlmGate>,
    pub settings: SettingsStore,
    pub bus: Arc<dyn SummaryBus>,
    pub clock: Arc<dyn Clock>,
}

/// Why a job could not finish.
#[derive(Debug)]
struct JobError {
    code: &'static str,
    message: String,
    /// Worth another job attempt later.
    transient: bool,
}

impl JobError {
    fn permanent(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            transient: false,
        }
    }
    fn transient(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            transient: true,
        }
    }
}

impl From<AppError> for JobError {
    fn from(e: AppError) -> Self {
        Self::transient(codes::INTERNAL, e.to_string())
    }
}

/// What a `run` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Another worker owns it, or it already reached a final state.
    Skipped,
    Completed,
    Failed(String),
    /// Transient failure; back to `pending` for the sweeper.
    Requeued,
}

#[derive(Default)]
struct Tally {
    input: u64,
    output: u64,
    calls: u32,
}

pub struct SummaryService {
    pub(crate) d: SummaryDeps,
    usage: Option<SharedUsageSink>,
    memory: Arc<dyn MeetingMemory>,
}

impl SummaryService {
    pub fn new(deps: SummaryDeps) -> Self {
        let memory = Arc::new(PgMeetingMemory::new(deps.repo.pool().clone()));
        Self {
            d: deps,
            usage: None,
            memory,
        }
    }

    /// Replace the default Postgres meeting memory (tests, other stores).
    pub fn with_memory(mut self, memory: Arc<dyn MeetingMemory>) -> Self {
        self.memory = memory;
        self
    }

    /// The cross-meeting memory this service feeds into summaries.
    pub fn memory(&self) -> Arc<dyn MeetingMemory> {
        self.memory.clone()
    }

    /// Prior context of `room` (empty when memory is disabled, the room has no known host, or
    /// the memory fails: it is best-effort background, never a reason to fail).
    pub async fn prior_context(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        cfg: &SummaryConfig,
        ctx: Option<&super::ports::RoomContext>,
    ) -> PriorContext {
        let Some(ctx) = ctx.filter(|_| cfg.memory_enabled) else {
            return PriorContext::default();
        };
        let before = match self.d.repo.first_created_at(room).await {
            Ok(Some(t)) => t,
            _ => self.d.clock.now(),
        };
        let q = PriorQuery {
            workspace_id: ws,
            room_id: room,
            host: ctx.host_user_id,
            series_id: ctx.series_id,
            title: Some(ctx.title.clone()).filter(|t| !t.trim().is_empty()),
            before,
            max_open_items: cfg.memory_max_open_items,
            max_summaries: cfg.memory_max_summaries,
        };
        self.memory.prior_context(&q).await.unwrap_or_else(|e| {
            tracing::warn!(%room, error = %e, "meeting memory unavailable; no prior context");
            PriorContext::default()
        })
    }

    /// Room context for the HTTP prior-context endpoint (config of the room's scope applied).
    pub async fn prior_context_for_room(
        &self,
        ws: WorkspaceId,
        room: RoomId,
    ) -> AppResult<PriorContext> {
        let cfg = self.config(ws, room).await;
        let ctx = self.d.rooms.context(room).await?;
        Ok(self.prior_context(ws, room, &cfg, ctx.as_ref()).await)
    }

    /// Publish a `usage:events` entry per completed summary (billing metering).
    pub fn with_usage(mut self, sink: SharedUsageSink) -> Self {
        self.usage = Some(sink);
        self
    }

    pub fn repo(&self) -> &PgSummaryRepository {
        &self.d.repo
    }

    pub async fn config(&self, ws: WorkspaceId, room: RoomId) -> SummaryConfig {
        self.d
            .settings
            .get::<SummaryConfig>(NS_SUMMARY, ScopeChain::room(ws, room))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(%room, error = %e, "summary settings unavailable; using defaults");
                SummaryConfig::default()
            })
    }

    /// `__MEETING_END__` seen for `room`: enqueue the automatic job (idempotent per transcript
    /// version). Returns the job to run, `None` when nothing is to be done (disabled, unknown
    /// room, no transcript, or the job already reached a final state).
    pub async fn on_meeting_end(&self, room: RoomId) -> AppResult<Option<SummaryJob>> {
        let Some(meta) = self.d.rooms.room(room).await? else {
            tracing::debug!(%room, "meeting end for an unknown room; no summary");
            return Ok(None);
        };
        let cfg = self.config(meta.workspace_id, room).await;
        if !cfg.enabled {
            return Ok(None);
        }
        let Some(t) = self.d.transcripts.load(room, 0).await? else {
            tracing::debug!(%room, "no transcript; no summary");
            return Ok(None);
        };
        let job = match self
            .d
            .repo
            .enqueue(
                meta.workspace_id,
                room,
                t.version,
                Trigger::Auto,
                None,
                self.d.clock.now(),
            )
            .await?
        {
            Enqueued::Created(j) => j,
            // A redelivered marker resumes an unfinished job and ignores a finished one.
            Enqueued::Existing(j) if j.status == SummaryStatus::Pending => j,
            Enqueued::Existing(_) => return Ok(None),
        };
        Ok(Some(job))
    }

    /// Explicit regenerate: the next version, announced on `assistant:summary_requests`.
    pub async fn regenerate(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        by: UserId,
    ) -> AppResult<SummaryJob> {
        if let Some(latest) = self.d.repo.latest(room).await?
            && matches!(
                latest.status,
                SummaryStatus::Pending | SummaryStatus::Running
            )
        {
            return Err(AppError::conflict(
                "SUMMARY_IN_PROGRESS",
                "a summary is already being generated for this meeting",
            ));
        }
        // Same gate as the job, checked up front: no row is created for a request that cannot
        // run (HTTP 400 `EXTERNAL_LLM_NOT_ALLOWED`). The job re-checks, so a policy change
        // between request and run is honoured. Resolution trouble is left to the job.
        if let Ok(r) = self.d.llm.for_purpose(ws, Purpose::Summary).await
            && r.boundary == DataBoundary::External
            && !self.d.policy.external_llm_allowed(ws).await
        {
            return Err(AppError::validation(
                codes::EXTERNAL_LLM_NOT_ALLOWED,
                "this workspace does not allow sending meeting data to external LLM providers",
            ));
        }
        let t = self
            .d
            .transcripts
            .load(room, 0)
            .await?
            .filter(|t| t.total_segments > 0)
            .ok_or_else(|| {
                AppError::validation(codes::NO_TRANSCRIPT, "this meeting has no saved transcript")
            })?;
        let job = match self
            .d
            .repo
            .enqueue(
                ws,
                room,
                t.version,
                Trigger::Manual,
                Some(by.as_uuid()),
                self.d.clock.now(),
            )
            .await?
        {
            Enqueued::Created(j) | Enqueued::Existing(j) => j,
        };
        let req = SummaryRequest {
            summary_id: job.id,
            room_id: room,
            workspace_id: ws,
        };
        if let Err(e) = self.d.bus.request(&req).await {
            // The sweeper still finds the pending row.
            tracing::warn!(%room, error = %e, "summary request not published");
        }
        Ok(job)
    }

    /// Pending or silent jobs (restart recovery, lost requests).
    ///
    /// `workspace` scopes the listing to one tenant (`None` = every workspace, the production
    /// sweeper); tests use it so they never touch rows of other tests sharing the database.
    pub async fn open_jobs(
        &self,
        quiet: Duration,
        stale: Duration,
        workspace: Option<WorkspaceId>,
    ) -> Vec<SummaryJob> {
        let q = ChronoDuration::from_std(quiet).unwrap_or_default();
        let s = ChronoDuration::from_std(stale).unwrap_or_default();
        self.d
            .repo
            .open_jobs(q, s, self.d.clock.now(), 50, workspace)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "listing open summary jobs failed");
                Vec::new()
            })
    }

    /// Claim and run one job to a final state (or back to `pending` on a transient failure).
    pub async fn run(&self, id: Uuid) -> RunOutcome {
        let Some(peek) = self.d.repo.get(id).await.ok().flatten() else {
            return RunOutcome::Skipped;
        };
        let cfg = self.config(peek.workspace_id, peek.room_id).await;
        let stale = ChronoDuration::seconds(cfg.stale_running_secs as i64);
        let job = match self.d.repo.claim(id, stale, self.d.clock.now()).await {
            Ok(Some(j)) => j,
            Ok(None) => return RunOutcome::Skipped,
            Err(e) => {
                tracing::warn!(summary = %id, error = %e, "claiming summary job failed");
                return RunOutcome::Skipped;
            }
        };
        let timer = StageTimer::start(STAGE);
        let result = self.generate(&job, &cfg).await;
        let ms = timer.finish();
        let req = SummaryRequest {
            summary_id: job.id,
            room_id: job.room_id,
            workspace_id: job.workspace_id,
        };
        match result {
            Ok(g) => {
                record_stage_outcome(STAGE, Outcome::Ok);
                metrics::counter!(TOKENS_METRIC, "kind" => "input").increment(g.tally.input);
                metrics::counter!(TOKENS_METRIC, "kind" => "output").increment(g.tally.output);
                if let (Some(sink), true) = (&self.usage, g.tally.calls > 0) {
                    // One event per job: re-running a job never charges twice.
                    sink.emit(UsageEvent::llm(
                        format!("llm_summary:{}", job.id),
                        "llm_summary",
                        job.workspace_id.as_uuid(),
                        job.room_id.as_uuid(),
                        g.model.clone(),
                        g.key_source,
                        g.tally.input,
                        g.tally.output,
                        None,
                    ))
                    .await;
                }
                let saved = self
                    .d
                    .repo
                    .complete(
                        &job,
                        &g.model,
                        &cfg.prompt_version,
                        &g.content,
                        g.content.meta.input_segments as i32,
                        (g.tally.input as i64, g.tally.output as i64),
                        self.d.clock.now(),
                    )
                    .await;
                if let Err(e) = saved {
                    return self
                        .settle_failure(&job, &cfg, &req, JobError::from(e))
                        .await;
                }
                tracing::info!(
                    room = %job.room_id, version = job.version, latency_ms = ms as u64,
                    llm_calls = g.tally.calls, tokens_in = g.tally.input,
                    tokens_out = g.tally.output, "meeting summary completed"
                );
                self.publish_result(&job, SummaryStatus::Completed, None)
                    .await;
                RunOutcome::Completed
            }
            Err(e) => {
                record_stage_outcome(
                    STAGE,
                    if e.transient {
                        Outcome::VendorError
                    } else {
                        Outcome::Error
                    },
                );
                self.settle_failure(&job, &cfg, &req, e).await
            }
        }
    }

    async fn settle_failure(
        &self,
        job: &SummaryJob,
        cfg: &SummaryConfig,
        req: &SummaryRequest,
        e: JobError,
    ) -> RunOutcome {
        let now = self.d.clock.now();
        if e.transient && job.attempts < cfg.max_job_attempts as i32 {
            tracing::warn!(room = %job.room_id, attempt = job.attempts, code = e.code,
                error = %e.message, "summary attempt failed; will retry");
            let _ = self.d.repo.requeue(job.id, &e.message, now).await;
            return RunOutcome::Requeued;
        }
        tracing::error!(room = %job.room_id, code = e.code, error = %e.message, "meeting summary failed");
        let _ = self.d.repo.fail(job.id, e.code, &e.message, now).await;
        if e.transient {
            record_stage_outcome(STAGE, Outcome::DeadLetter);
            self.d
                .bus
                .dead_letter(req, &format!("{}: {}", e.code, e.message))
                .await;
        }
        self.publish_result(job, SummaryStatus::Failed, Some(e.code.to_owned()))
            .await;
        RunOutcome::Failed(e.code.to_owned())
    }

    async fn publish_result(&self, job: &SummaryJob, status: SummaryStatus, code: Option<String>) {
        self.d
            .bus
            .result(&SummaryResultEvent {
                summary_id: job.id,
                room_id: job.room_id,
                workspace_id: job.workspace_id,
                version: job.version,
                status,
                error_code: code,
            })
            .await;
    }

    // ------------------------------------------------------------------ generation

    /// The marker can overtake the transcript consumer: wait until the transcript is finalized
    /// or its segment count stopped growing (two equal polls), at most `transcript_wait_secs`.
    async fn wait_for_transcript(
        &self,
        room: RoomId,
        cfg: &SummaryConfig,
    ) -> Result<Option<TranscriptData>, JobError> {
        let started = Instant::now();
        let poll = Duration::from_millis(cfg.transcript_poll_ms);
        let mut last: Option<u32> = None;
        loop {
            let head = self.d.transcripts.load(room, 0).await?;
            let settled = match &head {
                None => last == Some(0),
                Some(h) => h.finalized || last == Some(h.total_segments),
            };
            last = Some(head.as_ref().map_or(0, |h| h.total_segments));
            if settled || started.elapsed() >= Duration::from_secs(cfg.transcript_wait_secs) {
                return Ok(self.d.transcripts.load(room, cfg.max_segments).await?);
            }
            tokio::time::sleep(poll).await;
        }
    }

    async fn generate(&self, job: &SummaryJob, cfg: &SummaryConfig) -> Result<Generated, JobError> {
        let room = job.room_id;
        let t = self
            .wait_for_transcript(room, cfg)
            .await?
            .filter(|t| !t.segments.is_empty())
            .ok_or_else(|| JobError::permanent(codes::NO_TRANSCRIPT, "the transcript is empty"))?;
        let meta = self
            .d
            .rooms
            .room(room)
            .await?
            .ok_or_else(|| JobError::permanent(codes::ROOM_NOT_FOUND, "room not found"))?;
        let resolved = match self
            .d
            .llm
            .for_purpose(job.workspace_id, Purpose::Summary)
            .await
        {
            Ok(r) => r,
            Err(ResolveError::NotConfigured) => {
                return Err(JobError::permanent(
                    codes::LLM_NOT_CONFIGURED,
                    "no LLM provider is configured for summaries",
                ));
            }
            Err(ResolveError::Failed(m)) => {
                return Err(JobError::transient(codes::LLM_UNAVAILABLE, m));
            }
        };
        // Consent gate, before any transcript text leaves: an external provider needs the
        // workspace's (durable) opt-in; workspace-controlled endpoints are exempt.
        if resolved.boundary == DataBoundary::External
            && !self.d.policy.external_llm_allowed(job.workspace_id).await
        {
            return Err(JobError::permanent(
                codes::EXTERNAL_LLM_NOT_ALLOWED,
                "this workspace does not allow sending meeting data to external LLM providers",
            ));
        }
        let model = resolved.model.clone().unwrap_or_else(|| cfg.model.clone());
        let language = if cfg.language.trim().is_empty() {
            meta.source_language.clone()
        } else {
            cfg.language.trim().to_owned()
        };
        let followups = self.d.followups.for_room(room, cfg.max_followups).await;
        let room_ctx = self.d.rooms.context(room).await.unwrap_or_else(|e| {
            tracing::warn!(%room, error = %e, "room context unavailable; raw speaker names");
            None
        });
        let speakers = Speakers::from_context(room_ctx.as_ref());
        if let Some(c) = &room_ctx {
            // Lets the meeting memory find this meeting later (best effort).
            let _ = self
                .d
                .repo
                .stamp_room(
                    job.id,
                    c.host_user_id.map(|u| u.as_uuid()),
                    c.series_id,
                    &c.title,
                )
                .await;
        }
        let prior = self
            .prior_context(job.workspace_id, room, cfg, room_ctx.as_ref())
            .await;
        let prior_block =
            crate::memory::prompt::prior_block(&prior, cfg.memory_line_chars as usize);
        let prior_tasks: Vec<String> = prior
            .open_action_items
            .iter()
            .map(|i| i.task.clone())
            .collect();
        let segments = match &speakers {
            Some(sp) => sp.relabel(&t.segments),
            None => t.segments.clone(),
        };
        let ids: Vec<Uuid> = segments.iter().map(|s| s.id).collect();
        let hay = prompt::haystack_with(&segments, speakers.as_ref());
        let max_line = (cfg.window_tokens as f32 * cfg.chars_per_token) as usize;
        let lines = prompt::transcript_lines(&segments, max_line.max(200));
        let wins = prompt::windows(&lines, cfg.window_tokens, cfg.chars_per_token);

        let run = Run {
            provider: resolved.provider.clone(),
            model: model.clone(),
            cfg,
            lang: language.clone(),
            system: prompt::system_prompt_for(
                cfg,
                &language,
                speakers.as_ref(),
                !prior_block.is_empty(),
            ),
            tally: std::sync::Mutex::new(Tally::default()),
            repo: &self.d.repo,
            clock: self.d.clock.as_ref(),
            job_id: job.id,
        };
        let head_of = |f: &Fence, followups: &[LiveFollowUp]| {
            prompt::meta_block_with(&meta, followups, speakers.as_ref(), &prior_block, f)
        };
        let raw = if wins.len() == 1 {
            run.call(
                &prompt::single_instruction(cfg),
                |f| {
                    format!(
                        "{}\n{}",
                        head_of(f, &followups),
                        prompt::transcript_block(f, &wins[0])
                    )
                },
                "single",
            )
            .await?
        } else {
            self.map_reduce(&run, &wins, &|f| head_of(f, &[]), &|f| {
                head_of(f, &followups)
            })
            .await?
        };
        let mut content = prompt::validate_with(raw, &ids, &hay, speakers.as_ref(), &prior_tasks);
        content.language = language;
        let tally = run.tally.into_inner().unwrap_or_default();
        content.meta = SummaryMeta {
            input_segments: t.segments.len() as u32,
            total_segments: t.total_segments,
            truncated: t.total_segments as usize > t.segments.len(),
            windows: wins.len() as u32,
            llm_calls: tally.calls,
            live_followups_used: followups.len() as u32,
        };
        Ok(Generated {
            content,
            model,
            key_source: resolved.key_source,
            tally,
        })
    }

    async fn map_reduce(
        &self,
        run: &Run<'_>,
        wins: &[Vec<&str>],
        meta_head: &(dyn Fn(&Fence) -> String + Sync),
        meta_full: &(dyn Fn(&Fence) -> String + Sync),
    ) -> Result<RawOutput, JobError> {
        let cfg = run.cfg;
        let total = wins.len();
        let conc = cfg.map_concurrency.max(1) as usize;
        let mut parts: Vec<RawOutput> = Vec::with_capacity(total);
        for (ci, chunk) in wins.chunks(conc).enumerate() {
            let calls = chunk.iter().enumerate().map(|(j, w)| {
                let instruction = prompt::map_instruction(cfg, ci * conc + j + 1, total);
                async move {
                    run.call(
                        &instruction,
                        |f| format!("{}\n{}", meta_head(f), prompt::transcript_block(f, w)),
                        "map",
                    )
                    .await
                }
            });
            for r in futures::future::join_all(calls).await {
                parts.push(r?);
            }
        }

        let instruction = prompt::reduce_instruction(cfg);
        // Reduce hierarchically until the partial records fit one window.
        for _ in 0..4 {
            let groups = pack(&parts, cfg);
            if groups.len() <= 1 {
                break;
            }
            let mut next = Vec::with_capacity(groups.len());
            for chunk in groups.chunks(conc) {
                let calls = chunk.iter().map(|g| {
                    let instruction = &instruction;
                    async move {
                        run.call(
                            instruction,
                            |f| {
                                format!(
                                    "{}\nPartial records:\n{}",
                                    meta_head(f),
                                    partials_block(f, g)
                                )
                            },
                            "reduce",
                        )
                        .await
                    }
                });
                for r in futures::future::join_all(calls).await {
                    next.push(r?);
                }
            }
            parts = next;
        }
        run.call(
            &instruction,
            |f| {
                format!(
                    "{}\nPartial records:\n{}",
                    meta_full(f),
                    partials_block(f, &parts)
                )
            },
            "reduce",
        )
        .await
    }
}

struct Generated {
    content: SummaryContent,
    model: String,
    key_source: crate::suggestion::KeySource,
    tally: Tally,
}

/// Each partial record in its own fence (they are model output derived from untrusted text).
fn partials_block(fence: &Fence, parts: &[RawOutput]) -> String {
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            fence.wrap(
                &format!("partial_{i}"),
                &partials_json(std::slice::from_ref(p)),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn partials_json(parts: &[RawOutput]) -> String {
    let v: Vec<serde_json::Value> = parts
        .iter()
        .map(|p| {
            serde_json::json!({
                "summary_short": p.summary_short,
                "summary_detailed": p.summary_detailed,
                "decisions": p.decisions,
                "action_items": p.action_items.iter().map(|a| serde_json::json!({
                    "task": a.task, "owner": a.owner, "due": a.due,
                    "source_segments": a.source_segments,
                })).collect::<Vec<_>>(),
                "open_questions": p.open_questions,
                "topics": p.topics,
                "previous_items": p.previous_items.iter().map(|x| serde_json::json!({
                    "task": x.task, "status": x.status, "source_segments": x.source_segments,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string(&v).unwrap_or_default()
}

/// Consecutive partials grouped so each group fits `window_tokens`.
fn pack(parts: &[RawOutput], cfg: &SummaryConfig) -> Vec<Vec<RawOutput>> {
    let mut groups: Vec<Vec<RawOutput>> = Vec::new();
    let mut cur: Vec<RawOutput> = Vec::new();
    let mut used = 0u32;
    for p in parts {
        let t =
            prompt::estimate_tokens(&partials_json(std::slice::from_ref(p)), cfg.chars_per_token);
        if !cur.is_empty() && used + t > cfg.window_tokens {
            groups.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(p.clone());
        used += t;
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    groups
}

/// One job's LLM access: retries, backoff, accounting.
struct Run<'a> {
    provider: Arc<dyn LlmProvider>,
    model: String,
    cfg: &'a SummaryConfig,
    system: String,
    lang: String,
    tally: std::sync::Mutex<Tally>,
    repo: &'a PgSummaryRepository,
    clock: &'a dyn Clock,
    job_id: Uuid,
}

impl Run<'_> {
    fn request(&self, instruction: &str, user: &str, stage: &str, fence: &Fence) -> Request {
        let mut req = Request::new(
            self.model.clone(),
            vec![Message::user(format!("{instruction}\n\n{user}"))],
        );
        req.system = Some(format!(
            "{}\n\n{}",
            self.system,
            safety::untrusted_rule(safety::Lang::from_code(&self.lang), fence)
        ));
        req.temperature = Some(self.cfg.temperature);
        req.max_output_tokens = Some(self.cfg.max_output_tokens);
        req.response_format = Some(JsonSchemaFormat {
            name: "meeting_summary".into(),
            schema: output_schema(),
            strict: true,
        });
        req.metadata.insert("purpose".into(), "summary".into());
        req.metadata.insert("stage".into(), stage.into());
        req
    }

    fn backoff(&self, attempt: u32, hint: Option<Duration>) -> Duration {
        let exp = self
            .cfg
            .backoff_base_ms
            .saturating_mul(1u64 << attempt.saturating_sub(1).min(10));
        let ms = exp.min(self.cfg.backoff_max_ms);
        hint.map_or(Duration::from_millis(ms), |h| {
            h.min(Duration::from_millis(self.cfg.backoff_max_ms))
        })
    }

    async fn call(
        &self,
        instruction: &str,
        user: impl FnOnce(&Fence) -> String,
        stage: &str,
    ) -> Result<RawOutput, JobError> {
        // One fence per LLM call (shared by its retries).
        let fence = Fence::new();
        let user = user(&fence);
        let req = self.request(instruction, &user, stage, &fence);
        let mut last = JobError::transient(codes::LLM_UNAVAILABLE, "no attempt made");
        for attempt in 1..=self.cfg.max_attempts {
            let timeout = Duration::from_secs(self.cfg.llm_timeout_secs);
            let res = tokio::time::timeout(timeout, self.provider.complete(&req)).await;
            let wait = match res {
                Err(_) => {
                    last = JobError::transient(codes::LLM_UNAVAILABLE, "LLM request timed out");
                    self.backoff(attempt, None)
                }
                Ok(Err(e)) if e.is_retryable() => {
                    last = JobError::transient(codes::LLM_UNAVAILABLE, e.to_string());
                    self.backoff(attempt, e.retry_after())
                }
                Ok(Err(e)) => return Err(permanent_llm_error(&e)),
                Ok(Ok(resp)) => {
                    if let Ok(mut t) = self.tally.lock() {
                        t.calls += 1;
                        t.input += u64::from(
                            resp.usage.input_uncached
                                + resp.usage.cache_read
                                + resp.usage.cache_write,
                        );
                        t.output += u64::from(resp.usage.output + resp.usage.reasoning);
                    }
                    let _ = self.repo.touch(self.job_id, self.clock.now()).await;
                    let text = resp.text();
                    match serde_json::from_str::<RawOutput>(prompt::strip_fence(&text)) {
                        Ok(raw) => return Ok(raw),
                        Err(e) => {
                            last = JobError::permanent(
                                codes::LLM_BAD_OUTPUT,
                                format!("unparseable model reply: {e}"),
                            );
                            Duration::ZERO
                        }
                    }
                }
            };
            if attempt < self.cfg.max_attempts {
                tokio::time::sleep(wait).await;
            }
        }
        Err(last)
    }
}

fn permanent_llm_error(e: &LlmError) -> JobError {
    JobError::permanent(codes::LLM_FAILED, e.to_string())
}
