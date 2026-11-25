//! `ChatService`: gate, gather context, call the LLM (streaming), cite, persist, meter.
//!
//! The HTTP layer checks room access, then calls [`ChatService::ask`] / [`ChatService::recap`]
//! to get a [`Prepared`] (every gate error is a plain `AppError`, so it becomes a JSON error
//! status before any SSE starts) and [`ChatService::execute`] to run it as a stream of
//! [`ChatEvent`]s.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, UserId, WorkspaceId};
use warptalk_llm::{LlmError, LlmProvider, Message, Part, Request, Role, StreamEvent, Usage};
use warptalk_settings::{ScopeChain, SettingsStore};

use super::config::{ChatConfig, NS_CHAT};
use super::model::{AskRequest, ChatEvent, ChatScope, Citation, RecapRequest, codes};
use super::ports::{CachedRecap, PriorContextSource, RateLimiter, RecapCache};
use super::prompt::{self, RefMap, Section};
use super::repo::{Exchange, PgChatRepository};
use crate::safety::{Fence, Lang, OutputPolicy, check_output};
use crate::suggestion::{
    DataBoundary, GroundedSnippet, KeySource, KnowledgeQuery, KnowledgeRetriever, LlmResolver,
    Purpose, ResolveError,
};
use crate::summary::{
    ExternalLlmGate, RoomMetaSource, TranscriptData, TranscriptSegment, TranscriptSource,
};
use crate::usage::{SharedUsageSink, UsageEvent};

/// Usage feature label of chat calls.
pub const FEATURE: &str = "llm_assistant";

pub struct ChatDeps {
    pub repo: PgChatRepository,
    pub transcripts: Arc<dyn TranscriptSource>,
    pub rooms: Arc<dyn RoomMetaSource>,
    pub knowledge: Arc<dyn KnowledgeRetriever>,
    pub prior: Arc<dyn PriorContextSource>,
    pub llm: Arc<dyn LlmResolver>,
    /// Workspace `externalLlmAllowed` policy, applied to `External` providers.
    pub policy: Arc<dyn ExternalLlmGate>,
    pub settings: SettingsStore,
    pub limiter: Arc<dyn RateLimiter>,
    pub recap_cache: Arc<dyn RecapCache>,
    pub usage: SharedUsageSink,
    pub clock: Arc<dyn Clock>,
}

/// An answer that needs no LLM call (nothing relevant, or a cached recap).
struct Ready {
    ctx: Ctx,
    answer: String,
    citations: Vec<Citation>,
    model: String,
    cached: bool,
}

/// A call to run.
struct LlmJob {
    ctx: Ctx,
    request: Request,
    provider: Arc<dyn LlmProvider>,
    model: String,
    key_source: KeySource,
    refs: RefMap,
    timeout: Duration,
    prompt_chars: usize,
    chars_per_token: f32,
    cache: Option<(String, Duration)>,
    /// Grounding that may legitimately carry URLs (snippets, earlier-meeting notes).
    trusted: Vec<String>,
    /// Transcript lines that were in the prompt.
    transcript_lines: Vec<String>,
}

/// Who asked what (stored with the exchange).
#[derive(Clone)]
struct Ctx {
    workspace: WorkspaceId,
    room: RoomId,
    user: UserId,
    /// `ask` | `recap`.
    kind: &'static str,
    /// Text stored as the user's message.
    question: String,
    scope: &'static str,
}

/// Everything decided before the first byte is streamed.
pub struct Prepared(PreparedInner);

enum PreparedInner {
    Ready(Box<Ready>),
    Llm(Box<LlmJob>),
}

pub struct ChatService {
    d: ChatDeps,
}

impl ChatService {
    pub fn new(deps: ChatDeps) -> Self {
        Self { d: deps }
    }

    pub fn repo(&self) -> &PgChatRepository {
        &self.d.repo
    }

    pub async fn config(&self, ws: WorkspaceId, room: RoomId) -> ChatConfig {
        self.d
            .settings
            .get::<ChatConfig>(NS_CHAT, ScopeChain::room(ws, room))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(%room, error = %e, "chat settings unavailable; using defaults");
                ChatConfig::default()
            })
    }

    async fn gate(
        &self,
        ws: WorkspaceId,
        cfg: &ChatConfig,
    ) -> AppResult<crate::suggestion::ResolvedLlm> {
        ensure_enabled(cfg)?;
        let resolved = match self.d.llm.for_purpose(ws, Purpose::Summary).await {
            Ok(r) => r,
            Err(ResolveError::NotConfigured) => {
                return Err(AppError::coded(
                    409,
                    codes::LLM_NOT_CONFIGURED,
                    "no LLM provider is configured for the assistant",
                ));
            }
            Err(ResolveError::Failed(m)) => {
                tracing::warn!(error = %m, "chat provider resolution failed");
                return Err(AppError::unavailable("the LLM provider is unavailable"));
            }
        };
        // Consent gate before any meeting text is read for the call.
        if resolved.boundary == DataBoundary::External
            && !self.d.policy.external_llm_allowed(ws).await
        {
            return Err(AppError::coded(
                403,
                codes::EXTERNAL_LLM_NOT_ALLOWED,
                "this workspace does not allow sending meeting data to external LLM providers",
            ));
        }
        Ok(resolved)
    }

    async fn rate_limit(&self, user: UserId, cfg: &ChatConfig) -> AppResult<()> {
        let ok = self
            .d
            .limiter
            .allow(
                &user.to_string(),
                cfg.rate_limit_max,
                Duration::from_secs(u64::from(cfg.rate_limit_window_secs)),
            )
            .await;
        if ok {
            Ok(())
        } else {
            Err(AppError::rate_limited(
                "too many assistant requests; try again shortly",
            ))
        }
    }

    async fn load_transcript(
        &self,
        room: RoomId,
        cfg: &ChatConfig,
    ) -> AppResult<Option<TranscriptData>> {
        let t = self.d.transcripts.load(room, cfg.max_segments).await?;
        if let Some(t) = &t
            && t.total_segments > cfg.max_segments
        {
            tracing::warn!(%room, total = t.total_segments, max = cfg.max_segments,
                "transcript longer than assistant_chat.max_segments; newest lines are not visible");
        }
        Ok(t)
    }

    /// Prepare an answer to a question.
    pub async fn ask(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        user: UserId,
        req: AskRequest,
    ) -> AppResult<Prepared> {
        let cfg = self.config(ws, room).await;
        let question = req.question.trim().to_owned();
        if question.is_empty() {
            return Err(AppError::validation(
                codes::QUESTION_EMPTY,
                "question must not be empty",
            ));
        }
        if question.chars().count() > cfg.max_question_chars as usize {
            return Err(AppError::validation(
                codes::QUESTION_TOO_LONG,
                format!(
                    "question must be at most {} characters",
                    cfg.max_question_chars
                ),
            ));
        }
        if req.since_ms.is_some_and(|s| s < 0) {
            return Err(AppError::validation(
                codes::INVALID_SINCE,
                "since_ms must be >= 0",
            ));
        }
        let scope = req.scope.unwrap_or_default();
        ensure_enabled(&cfg)?;
        self.rate_limit(user, &cfg).await?;
        let resolved = self.gate(ws, &cfg).await?;
        let model = resolved.model.clone().unwrap_or_else(|| cfg.model.clone());

        let transcript = self.load_transcript(room, &cfg).await?;
        let segments: Vec<TranscriptSegment> = transcript
            .map(|t| t.segments)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| req.since_ms.is_none_or(|since| s.start_ms >= since))
            .collect();

        let snippets = if scope == ChatScope::Meeting {
            Vec::new()
        } else {
            let top_k = if scope == ChatScope::Workspace {
                cfg.workspace_top_k
            } else {
                cfg.doc_top_k
            };
            self.snippets(ws, room, &question, top_k).await
        };
        let prior = self
            .d
            .prior
            .prior_context(ws, room, user, &question, cfg.chars_of(cfg.prior_tokens))
            .await;

        let ctx = Ctx {
            workspace: ws,
            room,
            user,
            kind: "ask",
            question: question.clone(),
            scope: scope.as_str(),
        };
        if segments.is_empty()
            && snippets.iter().all(|s| s.text.trim().is_empty())
            && prior.is_empty()
        {
            let msg = if prompt::looks_vietnamese(&question) {
                cfg.not_found_message_vi.clone()
            } else {
                cfg.not_found_message_en.clone()
            };
            return Ok(Prepared(PreparedInner::Ready(Box::new(Ready {
                ctx,
                answer: msg,
                citations: Vec::new(),
                model,
                cached: false,
            }))));
        }

        let fence = Fence::new();
        let mut refs = RefMap::default();
        let (earlier, recent) = prompt::select_for_question(&segments, &question, &cfg);
        let transcript_lines: Vec<String> = earlier
            .iter()
            .chain(recent.iter())
            .map(|s| s.text.clone())
            .collect();
        let trusted: Vec<String> = snippets
            .iter()
            .flat_map(|s| [s.title.clone(), s.text.clone()])
            .chain(prior.iter().map(|p| p.text.clone()))
            .collect();
        let lang = if prompt::looks_vietnamese(&question) {
            Lang::Vi
        } else {
            Lang::En
        };
        let line_max = cfg.max_line_chars as usize;
        let mut blocks = Vec::new();
        if let Some(info) = self.meeting_info(room).await {
            blocks.push(fence.wrap("meeting_info", &info));
        }
        let transcript_text = prompt::render_transcript(
            &[
                Section {
                    heading: "Earlier excerpts (picked by relevance)",
                    segments: earlier,
                },
                Section {
                    heading: "Most recent",
                    segments: recent,
                },
            ],
            line_max,
            &mut refs,
        );
        if transcript_text.is_empty() {
            blocks.push("(no transcript in scope)".to_owned());
        } else {
            blocks.push(fence.wrap("transcript", &transcript_text));
        }
        let docs = prompt::render_docs(&snippets, cfg.chars_of(cfg.docs_tokens), &mut refs);
        if !docs.is_empty() {
            blocks.push(fence.wrap("documents", &docs));
        }
        let prior_text = prompt::render_prior(&prior, cfg.chars_of(cfg.prior_tokens));
        if !prior_text.is_empty() {
            blocks.push(fence.wrap("earlier_meetings", &prior_text));
        }
        let user_msg = format!("{}\n\nQuestion:\n{}", blocks.join("\n\n"), question);

        let mut messages = self.history_messages(&ctx, &cfg).await;
        messages.push(Message::user(user_msg));
        let system = prompt::system_ask(&fence, lang);
        let prompt_chars = system.len() + messages.iter().map(msg_chars).sum::<usize>();
        let mut request = Request::new(model.clone(), messages);
        request.system = Some(system);
        request.temperature = Some(cfg.temperature);
        request.max_output_tokens = Some(cfg.max_output_tokens);
        request.metadata.insert("stage".into(), "chat_ask".into());
        request.metadata.insert("user_id".into(), user.to_string());
        Ok(Prepared(PreparedInner::Llm(Box::new(LlmJob {
            ctx,
            request,
            provider: resolved.provider,
            model,
            key_source: resolved.key_source,
            refs,
            timeout: Duration::from_secs(cfg.llm_timeout_secs),
            prompt_chars,
            chars_per_token: cfg.chars_per_token,
            cache: None,
            trusted,
            transcript_lines,
        }))))
    }

    /// Prepare a recap of the last `last_minutes` of the meeting (all of it when `None`).
    pub async fn recap(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        user: UserId,
        req: RecapRequest,
    ) -> AppResult<Prepared> {
        let cfg = self.config(ws, room).await;
        if let Some(m) = req.last_minutes
            && !cfg.recap_windows.contains(&m)
        {
            return Err(AppError::validation(
                codes::INVALID_WINDOW,
                format!(
                    "last_minutes must be null or one of {:?}",
                    cfg.recap_windows
                ),
            ));
        }
        let resolved = self.gate(ws, &cfg).await?;
        let model = resolved.model.clone().unwrap_or_else(|| cfg.model.clone());
        let meta = self.d.rooms.room(room).await?;
        let language = req
            .language
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty() && l.chars().count() <= 32)
            .map(str::to_owned)
            .or_else(|| meta.as_ref().map(|m| m.source_language.clone()))
            .unwrap_or_else(|| "vi".into());
        let window = req
            .last_minutes
            .map_or_else(|| "all".to_owned(), |m| m.to_string());
        let ctx = Ctx {
            workspace: ws,
            room,
            user,
            kind: "recap",
            question: format!("recap:{window}"),
            scope: "meeting",
        };

        let segments = self
            .load_transcript(room, &cfg)
            .await?
            .map(|t| t.segments)
            .unwrap_or_default();
        let Some(last) = segments.last() else {
            let msg = if language.starts_with("vi") {
                cfg.not_found_message_vi.clone()
            } else {
                cfg.not_found_message_en.clone()
            };
            return Ok(Prepared(PreparedInner::Ready(Box::new(Ready {
                ctx,
                answer: msg,
                citations: Vec::new(),
                model,
                cached: false,
            }))));
        };
        // "Window end" = the newest line: a new line produces a new key, an idle meeting hits.
        let end_ms = last.start_ms;
        let cache_key = format!(
            "assistant:chat:recap:{ws}:{room}:{window}:{language}:{}:{}",
            last.id,
            segments.len()
        );
        if cfg.recap_cache_secs > 0
            && let Some(hit) = self.d.recap_cache.get(&cache_key).await
        {
            return Ok(Prepared(PreparedInner::Ready(Box::new(Ready {
                ctx,
                answer: hit.answer,
                citations: hit.citations,
                model: hit.model,
                cached: true,
            }))));
        }
        self.rate_limit(user, &cfg).await?;

        let split = match req.last_minutes {
            Some(m) => {
                let from = end_ms - i64::from(m) * 60_000;
                segments.partition_point(|s| s.start_ms < from)
            }
            None => 0,
        };
        let (before, window_segs) = segments.split_at(split);
        // Earlier part: only the newest lines that fit the recent budget, for open questions and
        // decisions; the recap bullets come from the window.
        let mut budget = cfg.recent_tokens;
        let mut from = before.len();
        for (i, s) in before.iter().enumerate().rev() {
            let c = cfg.tokens_of(s.text.chars().count().min(cfg.max_line_chars as usize) + 24);
            if c > budget {
                break;
            }
            budget -= c;
            from = i;
        }
        // The window itself is capped by the same budget (newest lines win).
        let mut wbudget = cfg.recent_tokens.saturating_mul(2);
        let mut wfrom = window_segs.len();
        for (i, s) in window_segs.iter().enumerate().rev() {
            let c = cfg.tokens_of(s.text.chars().count().min(cfg.max_line_chars as usize) + 24);
            if c > wbudget {
                break;
            }
            wbudget -= c;
            wfrom = i;
        }
        let fence = Fence::new();
        let mut refs = RefMap::default();
        let mut blocks = Vec::new();
        if let Some(m) = &meta {
            let mut info = format!("Title: {}", prompt::defang(m.title.trim()));
            if !m.participants.is_empty() {
                info.push_str(&format!(
                    "\nParticipants: {}",
                    prompt::defang(&m.participants.join(", "))
                ));
            }
            blocks.push(fence.wrap("meeting_info", &info));
        }
        let text = prompt::render_transcript(
            &[
                Section {
                    heading: "Earlier in the meeting (context for open questions and decisions only)",
                    segments: before[from..].iter().collect(),
                },
                Section {
                    heading: "RECAP WINDOW",
                    segments: window_segs[wfrom..].iter().collect(),
                },
            ],
            cfg.max_line_chars as usize,
            &mut refs,
        );
        blocks.push(fence.wrap("transcript", &text));
        let transcript_lines: Vec<String> = before[from..]
            .iter()
            .chain(window_segs[wfrom..].iter())
            .map(|s| s.text.clone())
            .collect();
        let user_msg = format!(
            "{}\n\nWrite the recap now (window: {}).",
            blocks.join("\n\n"),
            req.last_minutes.map_or_else(
                || "whole meeting".to_owned(),
                |m| format!("last {m} minutes")
            )
        );
        let system = prompt::system_recap(&fence, Lang::from_code(&language), &language);
        let prompt_chars = system.len() + user_msg.len();
        let mut request = Request::new(model.clone(), vec![Message::user(user_msg)]);
        request.system = Some(system);
        request.temperature = Some(cfg.temperature);
        request.max_output_tokens = Some(cfg.recap_max_output_tokens);
        request.metadata.insert("stage".into(), "chat_recap".into());
        request.metadata.insert("user_id".into(), user.to_string());
        Ok(Prepared(PreparedInner::Llm(Box::new(LlmJob {
            ctx,
            request,
            provider: resolved.provider,
            model,
            key_source: resolved.key_source,
            refs,
            timeout: Duration::from_secs(cfg.llm_timeout_secs),
            prompt_chars,
            chars_per_token: cfg.chars_per_token,
            cache: (cfg.recap_cache_secs > 0).then(|| {
                (
                    cache_key,
                    Duration::from_secs(u64::from(cfg.recap_cache_secs)),
                )
            }),
            trusted: Vec::new(),
            transcript_lines,
        }))))
    }

    async fn snippets(
        &self,
        ws: WorkspaceId,
        room: RoomId,
        question: &str,
        top_k: u32,
    ) -> Vec<GroundedSnippet> {
        if top_k == 0 {
            return Vec::new();
        }
        match self
            .d
            .knowledge
            .retrieve(&KnowledgeQuery {
                workspace_id: ws,
                room_id: room,
                query: question.to_owned(),
                top_k,
            })
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%room, error = %e, "chat knowledge retrieval failed; continuing without documents");
                Vec::new()
            }
        }
    }

    async fn meeting_info(&self, room: RoomId) -> Option<String> {
        let m = self.d.rooms.room(room).await.ok().flatten()?;
        let mut s = format!("Title: {}", prompt::defang(m.title.trim()));
        if !m.participants.is_empty() {
            s.push_str(&format!(
                "\nParticipants: {}",
                prompt::defang(&m.participants.join(", "))
            ));
        }
        Some(s)
    }

    /// The user's last few exchanges in this room, as chat turns.
    async fn history_messages(&self, ctx: &Ctx, cfg: &ChatConfig) -> Vec<Message> {
        if cfg.history_turns == 0 {
            return Vec::new();
        }
        let rows = self
            .d
            .repo
            .history(
                ctx.workspace,
                ctx.room,
                ctx.user,
                Some("ask"),
                None,
                cfg.history_turns * 2,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "chat history unavailable; answering without it");
                Vec::new()
            });
        rows.into_iter()
            .map(|r| {
                let role = if r.role == "user" {
                    Role::User
                } else {
                    Role::Assistant
                };
                Message::text(role, r.content)
            })
            .collect()
    }

    /// Run a prepared answer; events arrive on the returned channel. The work continues after
    /// the receiver is dropped only up to the next send.
    pub fn execute(self: &Arc<Self>, prepared: Prepared) -> mpsc::Receiver<ChatEvent> {
        let (tx, rx) = mpsc::channel(64);
        let svc = self.clone();
        tokio::spawn(async move {
            match prepared.0 {
                PreparedInner::Ready(r) => svc.run_ready(*r, tx).await,
                PreparedInner::Llm(j) => svc.run_llm(*j, tx).await,
            }
        });
        rx
    }

    async fn persist(
        &self,
        ctx: &Ctx,
        answer: &str,
        citations: &[Citation],
        model: &str,
    ) -> Option<Uuid> {
        match self
            .d
            .repo
            .save_exchange(&Exchange {
                workspace: ctx.workspace,
                room: ctx.room,
                user: ctx.user,
                kind: ctx.kind,
                question: &ctx.question,
                answer,
                citations,
                model,
                scope: ctx.scope,
                at: self.d.clock.now(),
            })
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(room = %ctx.room, error = %e, "could not store chat history");
                None
            }
        }
    }

    async fn run_ready(&self, r: Ready, tx: mpsc::Sender<ChatEvent>) {
        if tx.send(ChatEvent::Delta(r.answer.clone())).await.is_err() {
            return;
        }
        if tx
            .send(ChatEvent::Citations(r.citations.clone()))
            .await
            .is_err()
        {
            return;
        }
        let id = self
            .persist(&r.ctx, &r.answer, &r.citations, &r.model)
            .await;
        let _ = tx
            .send(ChatEvent::Done {
                model: r.model,
                message_id: id,
                cached: r.cached,
            })
            .await;
    }

    async fn run_llm(&self, job: LlmJob, tx: mpsc::Sender<ChatEvent>) {
        let fail = |code: &str, message: &str| ChatEvent::Error {
            code: code.to_owned(),
            message: message.to_owned(),
        };
        let started = tokio::time::timeout(job.timeout, job.provider.stream(&job.request)).await;
        let mut stream = match started {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::warn!(room = %job.ctx.room, error = %e, "chat LLM call failed");
                let _ = tx.send(llm_error(&e)).await;
                return;
            }
            Err(_) => {
                let _ = tx
                    .send(fail(codes::LLM_UNAVAILABLE, "the LLM provider timed out"))
                    .await;
                return;
            }
        };
        let mut answer = String::new();
        let mut usage: Option<Usage> = None;
        let mut disconnected = false;
        let deadline = tokio::time::Instant::now() + job.timeout;
        let mut failed: Option<ChatEvent> = None;
        loop {
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            match next {
                Err(_) => {
                    failed = Some(fail(codes::LLM_UNAVAILABLE, "the LLM provider timed out"));
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    tracing::warn!(room = %job.ctx.room, error = %e, "chat LLM stream failed");
                    failed = Some(llm_error(&e));
                    break;
                }
                Ok(Some(Ok(StreamEvent::TextDelta(t)))) => {
                    if t.is_empty() {
                        continue;
                    }
                    answer.push_str(&t);
                    if tx.send(ChatEvent::Delta(t)).await.is_err() {
                        disconnected = true;
                        break;
                    }
                }
                Ok(Some(Ok(StreamEvent::Usage(u)))) => usage = Some(u),
                Ok(Some(Ok(StreamEvent::Done { .. }))) => break,
                Ok(Some(Ok(_))) => {}
            }
        }
        drop(stream);

        // The provider did the work: meter it even when the client left or the stream broke.
        self.meter(&job, usage.as_ref(), &answer).await;
        if disconnected {
            return;
        }
        if let Some(ev) = failed {
            let _ = tx.send(ev).await;
            return;
        }
        let answer = answer.trim().to_owned();
        if answer.is_empty() {
            let _ = tx
                .send(fail(codes::LLM_EMPTY, "the model returned no answer"))
                .await;
            return;
        }
        let trusted: Vec<&str> = job.trusted.iter().map(String::as_str).collect();
        let lines: Vec<&str> = job.transcript_lines.iter().map(String::as_str).collect();
        if let Err(v) = check_output(&[&answer], &trusted, &lines, &OutputPolicy::default()) {
            tracing::warn!(room = %job.ctx.room, violation = v.label(), "chat answer blocked by output policy");
            metrics::counter!("warptalk_chat_answers_blocked_total").increment(1);
            let _ = tx
                .send(fail(
                    codes::ANSWER_BLOCKED,
                    "the answer was withheld by the safety policy; discard any streamed text",
                ))
                .await;
            return;
        }
        let citations = prompt::citations(&answer, &job.refs);
        if tx
            .send(ChatEvent::Citations(citations.clone()))
            .await
            .is_err()
        {
            return;
        }
        let id = self
            .persist(&job.ctx, &answer, &citations, &job.model)
            .await;
        if let Some((key, ttl)) = &job.cache {
            self.d
                .recap_cache
                .put(
                    key,
                    &CachedRecap {
                        answer: answer.clone(),
                        citations: citations.clone(),
                        model: job.model.clone(),
                    },
                    *ttl,
                )
                .await;
        }
        let _ = tx
            .send(ChatEvent::Done {
                model: job.model,
                message_id: id,
                cached: false,
            })
            .await;
    }

    async fn meter(&self, job: &LlmJob, usage: Option<&Usage>, answer: &str) {
        let (tin, tout) = match usage {
            Some(u) => UsageEvent::tokens_of(u),
            None => (
                (job.prompt_chars as f32 / job.chars_per_token.max(0.5)).ceil() as u64,
                (answer.chars().count() as f32 / job.chars_per_token.max(0.5)).ceil() as u64,
            ),
        };
        let mut ev = UsageEvent::llm(
            format!("chat:{}", Uuid::now_v7()),
            FEATURE,
            job.ctx.workspace.as_uuid(),
            job.ctx.room.as_uuid(),
            job.model.clone(),
            job.key_source,
            tin,
            tout,
            None,
        );
        ev.user_id = Some(job.ctx.user.as_uuid());
        self.d.usage.emit(ev).await;
    }
}

fn ensure_enabled(cfg: &ChatConfig) -> AppResult<()> {
    if cfg.enabled {
        Ok(())
    } else {
        Err(AppError::coded(
            403,
            codes::CHAT_DISABLED,
            "the meeting assistant is turned off for this meeting",
        ))
    }
}

fn msg_chars(m: &Message) -> usize {
    m.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.len(),
            _ => 0,
        })
        .sum()
}

fn llm_error(e: &LlmError) -> ChatEvent {
    let (code, message) = match e {
        LlmError::Timeout | LlmError::Overloaded(_) | LlmError::RateLimited { .. } => (
            codes::LLM_UNAVAILABLE,
            "the LLM provider is busy, try again",
        ),
        _ => (codes::LLM_FAILED, "the assistant could not answer"),
    };
    ChatEvent::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}
