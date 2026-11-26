//! `bench-latency`: how long after a segment ends does a suggestion get published?
//!
//! The path is the one `SuggestionWorker` runs for every `stt:results` entry, minus the Redis
//! consumer hop: `SuggestionEngine::admit` inline, then a task that waits for a slot of the global
//! pipeline semaphore (the worker's `max_concurrent_pipelines`) and runs `SuggestionEngine::run`
//! (stage 0 -> decide -> retrieval -> generate -> hygiene -> publish to the Redis result stream).
//! Rooms replay a dataset meeting on a wall clock (optionally accelerated); the model is either
//! the scripted fake wrapped in a configurable latency distribution, or a real provider.
//!
//! Not measured: the STT engine, `POST /copilot/sessions/{id}/segments`, the `stt:results` hop and
//! the SSE push to the client. See `docs/perf/copilot-latency.md`.
//!
//! Per published suggestion: `total` = publish - segment arrival, `queue` = time until the model
//! pipeline started (admission, per-room slot, global semaphore), `llm` = sum of the model calls
//! of that segment, `other` = the rest (retrieval, hygiene, Redis).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use redis::aio::ConnectionManager;
use redis::streams::StreamRangeReply;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use warptalk_assistant::suggestion::engine::Admission;
use warptalk_assistant::suggestion::{
    AI_ASSISTANT_RESULTS, AllowAllConsent, EngineDeps, NoSignals, StaticConfig, SuggestionConfig,
    SuggestionEngine, SuggestionResult, SuggestionState,
};
use warptalk_kernel::{Clock, RoomId, WorkspaceId};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, ProviderId, Request, Response, StreamEvent,
};

use crate::dataset::Meeting;
use crate::harness::{EPOCH_MS_PUB, FixedResolver, OneRoom, SnippetRetriever, stt_for_line};
use crate::providers::ProviderSpec;
use crate::scoring::canon_category;

pub const BENCH_SCHEMA_VERSION: u32 = 1;
const STATE_PREFIX: &str = "evalbench";

// ------------------------------------------------------------------ latency distribution

/// Normal distribution in milliseconds (clamped to at least 5 ms when sampled).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Dist {
    pub mean_ms: f64,
    pub sd_ms: f64,
}

impl Dist {
    /// `MEAN` or `MEAN:SD` (milliseconds).
    pub fn parse(s: &str) -> Result<Self> {
        let (m, sd) = s.split_once(':').unwrap_or((s, "0"));
        let num = |x: &str| {
            x.trim()
                .parse::<f64>()
                .map_err(|_| anyhow!("bad latency `{s}` (use MEAN or MEAN:SD in ms)"))
        };
        Ok(Self {
            mean_ms: num(m)?,
            sd_ms: num(sd)?,
        })
    }
}

/// SplitMix64: tiny deterministic generator (no extra dependency).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self, d: Dist) -> f64 {
        let (u1, u2) = (self.unit(), self.unit());
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        (d.mean_ms + d.sd_ms * z).max(5.0)
    }
}

/// Adds a sampled delay (by call purpose) in front of an inner provider.
struct SimLatency {
    inner: Arc<dyn LlmProvider>,
    decide: Dist,
    generate: Dist,
    rng: Mutex<Rng>,
}

impl SimLatency {
    fn delay(&self, purpose: &str) -> Duration {
        let d = if purpose == "suggestion_decide" {
            self.decide
        } else {
            self.generate
        };
        let ms = self.rng.lock().map_or(d.mean_ms, |mut r| r.normal(d));
        Duration::from_secs_f64(ms / 1000.0)
    }
}

#[async_trait]
impl LlmProvider for SimLatency {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let purpose = req.metadata.get("purpose").map_or("", String::as_str);
        tokio::time::sleep(self.delay(purpose)).await;
        self.inner.complete(req).await
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        self.inner.stream(req).await
    }
    async fn validate(&self) -> Result<(), LlmError> {
        self.inner.validate().await
    }
}

// ------------------------------------------------------------------ call attribution

tokio::task_local! {
    /// `room/segment` of the pipeline running on this task; model calls are attributed to it.
    static CURRENT: String;
}

type CallLog = Arc<Mutex<HashMap<String, Vec<(String, f64)>>>>;

/// Measures every model call and attributes it to the segment whose pipeline issued it.
struct Attributing {
    inner: Arc<dyn LlmProvider>,
    log: CallLog,
}

#[async_trait]
impl LlmProvider for Attributing {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let started = Instant::now();
        let res = self.inner.complete(req).await;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let purpose = req
            .metadata
            .get("purpose")
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        if let Ok(key) = CURRENT.try_with(Clone::clone)
            && let Ok(mut log) = self.log.lock()
        {
            log.entry(key).or_default().push((purpose, ms));
        }
        res
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        self.inner.stream(req).await
    }
    async fn validate(&self) -> Result<(), LlmError> {
        self.inner.validate().await
    }
}

/// Wall clock scaled by the replay speed, so cooldowns and window ages behave as at 1x.
struct ScaledClock {
    start: Instant,
    speed: f64,
    offset_ms: AtomicI64,
}

impl Clock for ScaledClock {
    fn now(&self) -> DateTime<Utc> {
        let virt = (self.start.elapsed().as_secs_f64() * 1000.0 * self.speed) as i64;
        DateTime::<Utc>::from_timestamp_millis(
            EPOCH_MS_PUB + virt + self.offset_ms.load(Ordering::Relaxed),
        )
        .unwrap_or_else(Utc::now)
    }
}

// ------------------------------------------------------------------ report

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pcts {
    pub n: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

pub fn pcts(values: &[f64]) -> Pcts {
    if values.is_empty() {
        return Pcts::default();
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let pick = |q: f64| v[((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1];
    Pcts {
        n: v.len() as u64,
        mean_ms: v.iter().sum::<f64>() / v.len() as f64,
        p50_ms: pick(0.50),
        p95_ms: pick(0.95),
        p99_ms: pick(0.99),
        max_ms: v[v.len() - 1],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelReport {
    pub sessions: usize,
    pub segments_fed: u64,
    pub published: u64,
    pub outcomes: BTreeMap<String, u64>,
    pub wall_s: f64,
    /// Most pipelines waiting for a global slot at the same time.
    pub max_waiting: u64,
    /// Segment arrival -> suggestion published, all categories.
    pub total: Pcts,
    /// Same, `answer` category only.
    pub answer: Pcts,
    pub non_answer: Pcts,
    /// Arrival -> pipeline started (admission, per-room slot, global semaphore).
    pub queue: Pcts,
    /// Sum of the model calls of the segment.
    pub llm: Pcts,
    /// Retrieval, hygiene, Redis, scheduling.
    pub other: Pcts,
    pub answer_queue: Pcts,
    pub answer_llm: Pcts,
    pub decide_call: Pcts,
    pub generate_call: Pcts,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    pub schema_version: u32,
    pub created_at: String,
    pub provider: String,
    pub model: String,
    /// True when the model latency is the injected distribution, not a real network call.
    pub latency_simulated: bool,
    pub decide_ms: Option<Dist>,
    pub generate_ms: Option<Dist>,
    pub speed: f64,
    pub stagger_ms: i64,
    pub max_pipelines: usize,
    pub seed: u64,
    pub meetings: Vec<String>,
    pub levels: Vec<LevelReport>,
}

impl BenchReport {
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))
    }
}

fn row(o: &mut String, label: &str, p: &Pcts) {
    let _ = writeln!(
        o,
        "| {label} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
        p.n, p.mean_ms, p.p50_ms, p.p95_ms, p.p99_ms, p.max_ms
    );
}

pub fn to_markdown(r: &BenchReport) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Copilot latency bench\n");
    let model_lat = match (&r.decide_ms, &r.generate_ms) {
        (Some(d), Some(g)) if r.latency_simulated => format!(
            "simulated: decide {:.0}+-{:.0} ms, generate {:.0}+-{:.0} ms",
            d.mean_ms, d.sd_ms, g.mean_ms, g.sd_ms
        ),
        _ => "measured (real provider)".to_owned(),
    };
    let _ = writeln!(
        o,
        "- provider / model: `{}` / `{}` ({model_lat})\n- meetings: {}\n- replay speed: {}x, session start spread {} ms, global pipelines {}, seed {}\n- created: {}",
        r.provider,
        r.model,
        r.meetings.join(", "),
        r.speed,
        r.stagger_ms,
        r.max_pipelines,
        r.seed,
        r.created_at
    );
    for l in &r.levels {
        let _ = writeln!(
            o,
            "\n## {} concurrent session(s)\n\n{} segments fed, {} suggestions published, wall {:.1} s, max pipelines waiting for a slot {}.\n",
            l.sessions, l.segments_fed, l.published, l.wall_s, l.max_waiting
        );
        let _ = writeln!(
            o,
            "| segment end -> ... (ms) | n | mean | p50 | p95 | p99 | max |\n|---|---|---|---|---|---|---|"
        );
        row(&mut o, "published, all", &l.total);
        row(&mut o, "published, `answer`", &l.answer);
        row(&mut o, "published, other categories", &l.non_answer);
        row(&mut o, "queue time (all)", &l.queue);
        row(&mut o, "LLM time (all)", &l.llm);
        row(&mut o, "other (retrieval, hygiene, Redis)", &l.other);
        row(&mut o, "queue time (`answer`)", &l.answer_queue);
        row(&mut o, "LLM time (`answer`)", &l.answer_llm);
        row(&mut o, "single decide call", &l.decide_call);
        row(&mut o, "single generate call", &l.generate_call);
        let hist: Vec<String> = l.outcomes.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = writeln!(o, "\nOutcomes: {}", hist.join(", "));
    }
    o
}

// ------------------------------------------------------------------ run

pub struct BenchOptions {
    pub provider: ProviderSpec,
    pub model: String,
    pub meetings: Vec<Meeting>,
    /// Concurrent session counts to try, one level each.
    pub sessions: Vec<usize>,
    /// Replay speed (1 = real time). Model latency is not scaled, so speed > 1 raises the load.
    pub speed: f64,
    /// Sessions start spread evenly over this many (real) milliseconds.
    pub stagger_ms: i64,
    pub max_pipelines: usize,
    pub decide: Dist,
    pub generate: Dist,
    pub seed: u64,
    pub suggestion_overrides: Option<Value>,
    pub redis_url: String,
}

struct Sample {
    key: String,
    arrival: Instant,
    started: Instant,
    ended: Instant,
}

/// Build the provider stack of one meeting: model (fake + simulated latency, or real) wrapped by
/// the call attribution.
fn provider_for(o: &BenchOptions, m: &Meeting, log: &CallLog, salt: u64) -> Arc<dyn LlmProvider> {
    let base = o.provider.for_meeting(m);
    let inner: Arc<dyn LlmProvider> = if o.provider.is_fake() {
        Arc::new(SimLatency {
            inner: base,
            decide: o.decide,
            generate: o.generate,
            rng: Mutex::new(Rng(o.seed ^ salt.wrapping_mul(0x2545_F491_4F6C_DD1D))),
        })
    } else {
        base
    };
    Arc::new(Attributing {
        inner,
        log: log.clone(),
    })
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

async fn run_level(
    redis: &ConnectionManager,
    o: &BenchOptions,
    cfg: &SuggestionConfig,
    sessions: usize,
) -> Result<LevelReport> {
    let log: CallLog = Arc::new(Mutex::new(HashMap::new()));
    let start = Instant::now();
    let clock = Arc::new(ScaledClock {
        start,
        speed: o.speed,
        offset_ms: AtomicI64::new(0),
    });
    let ws = WorkspaceId::new();
    // One engine per meeting (the fake is scripted per meeting); rooms share it like the real
    // worker shares one engine across rooms.
    let engines: Vec<Arc<SuggestionEngine>> = o
        .meetings
        .iter()
        .enumerate()
        .map(|(i, m)| {
            Arc::new(SuggestionEngine::new(EngineDeps {
                rooms: Arc::new(OneRoom(ws)),
                signals: Arc::new(NoSignals),
                consent: Arc::new(AllowAllConsent),
                llm: Arc::new(FixedResolver {
                    provider: provider_for(o, m, &log, i as u64 + 1),
                    model: o.model.clone(),
                }),
                knowledge: Arc::new(SnippetRetriever(m.snippets.clone())),
                config: Arc::new(StaticConfig::new(cfg.clone())),
                clock: clock.clone(),
                state: SuggestionState::with_prefix(redis.clone(), STATE_PREFIX),
                redis: redis.clone(),
            }))
        })
        .collect();

    let global = Arc::new(Semaphore::new(o.max_pipelines.max(1)));
    let waiting = Arc::new(AtomicU64::new(0));
    let max_waiting = Arc::new(AtomicU64::new(0));
    let samples: Arc<Mutex<Vec<Sample>>> = Arc::new(Mutex::new(Vec::new()));
    let outcomes: Arc<Mutex<BTreeMap<String, u64>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let fed = Arc::new(AtomicU64::new(0));

    let rooms: Vec<(RoomId, usize)> = (0..sessions)
        .map(|k| (RoomId::new(), k % o.meetings.len()))
        .collect();
    let mut feeders = JoinSet::new();
    for (k, (room, mi)) in rooms.iter().copied().enumerate() {
        let (m, engine) = (o.meetings[mi].clone(), engines[mi].clone());
        let (global, waiting, max_waiting) = (global.clone(), waiting.clone(), max_waiting.clone());
        let (samples, outcomes, fed) = (samples.clone(), outcomes.clone(), fed.clone());
        let offset =
            Duration::from_millis((o.stagger_ms.max(0) as u64 * k as u64) / sessions as u64);
        let speed = o.speed;
        feeders.spawn(async move {
            let t0 = m.lines.first().map_or(0, |l| l.t_ms);
            let mut pipelines = JoinSet::new();
            for i in 0..m.lines.len() {
                let due = start
                    + offset
                    + Duration::from_secs_f64(
                        (m.lines[i].t_ms - t0).max(0) as f64 / 1000.0 / speed,
                    );
                tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
                let arrival = Instant::now();
                fed.fetch_add(1, Ordering::Relaxed);
                let stt = stt_for_line(&m, i, room);
                let key = format!("{room}/{}", stt.segment_id);
                match engine.admit(&stt).await {
                    Admission::Rejected(out) => {
                        if let Ok(mut h) = outcomes.lock() {
                            *h.entry(out.label().to_owned()).or_default() += 1;
                        }
                    }
                    Admission::Admitted(a) => {
                        let (engine, global) = (engine.clone(), global.clone());
                        let (waiting, max_waiting) = (waiting.clone(), max_waiting.clone());
                        let (samples, outcomes) = (samples.clone(), outcomes.clone());
                        pipelines.spawn(async move {
                            let w = waiting.fetch_add(1, Ordering::SeqCst) + 1;
                            max_waiting.fetch_max(w, Ordering::SeqCst);
                            let permit = global.acquire_owned().await;
                            waiting.fetch_sub(1, Ordering::SeqCst);
                            let Ok(_permit) = permit else { return };
                            let started = Instant::now();
                            let out = CURRENT.scope(key.clone(), engine.run(*a)).await;
                            let ended = Instant::now();
                            if let Ok(mut h) = outcomes.lock() {
                                *h.entry(out.label().to_owned()).or_default() += 1;
                            }
                            if out.is_published()
                                && let Ok(mut s) = samples.lock()
                            {
                                s.push(Sample {
                                    key,
                                    arrival,
                                    started,
                                    ended,
                                });
                            }
                        });
                    }
                }
            }
            while pipelines.join_next().await.is_some() {}
            engine.forget_room(room);
        });
    }
    let deadline = Duration::from_secs(900);
    let joined = tokio::time::timeout(deadline, async {
        while feeders.join_next().await.is_some() {}
    })
    .await;
    if joined.is_err() {
        bail!("level with {sessions} sessions did not finish within {deadline:?}");
    }
    let wall_s = start.elapsed().as_secs_f64();

    // Category of each published suggestion, from the result stream.
    let mut category: HashMap<String, String> = HashMap::new();
    for (room, _) in &rooms {
        let key = format!("{AI_ASSISTANT_RESULTS}:{room}");
        for r in read_results(redis, &key).await? {
            category.insert(
                format!("{room}/{}", r.segment_id),
                canon_category(&r.category),
            );
        }
        let mut conn = redis.clone();
        let _: Result<i64, _> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
    }

    let samples = samples.lock().map_err(|_| anyhow!("poisoned"))?;
    let calls = log.lock().map_err(|_| anyhow!("poisoned"))?;
    let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1000.0;
    let (mut total, mut answer, mut non_answer) = (vec![], vec![], vec![]);
    let (mut queue, mut llm, mut other) = (vec![], vec![], vec![]);
    let (mut answer_queue, mut answer_llm) = (vec![], vec![]);
    for s in samples.iter() {
        let t = ms(s.arrival, s.ended);
        let q = ms(s.arrival, s.started);
        let l: f64 = calls
            .get(&s.key)
            .map_or(0.0, |v| v.iter().map(|(_, d)| d).sum());
        total.push(t);
        queue.push(q);
        llm.push(l);
        other.push((t - q - l).max(0.0));
        if category.get(&s.key).is_some_and(|c| c == "answer") {
            answer.push(t);
            answer_queue.push(q);
            answer_llm.push(l);
        } else {
            non_answer.push(t);
        }
    }
    let by_purpose = |p: &str| -> Vec<f64> {
        calls
            .values()
            .flatten()
            .filter(|(k, _)| k == p)
            .map(|(_, d)| *d)
            .collect()
    };
    Ok(LevelReport {
        sessions,
        segments_fed: fed.load(Ordering::Relaxed),
        published: samples.len() as u64,
        outcomes: outcomes.lock().map_err(|_| anyhow!("poisoned"))?.clone(),
        wall_s,
        max_waiting: max_waiting.load(Ordering::Relaxed),
        total: pcts(&total),
        answer: pcts(&answer),
        non_answer: pcts(&non_answer),
        queue: pcts(&queue),
        llm: pcts(&llm),
        other: pcts(&other),
        answer_queue: pcts(&answer_queue),
        answer_llm: pcts(&answer_llm),
        decide_call: pcts(&by_purpose("suggestion_decide")),
        generate_call: pcts(&by_purpose("suggestion_generate")),
    })
}

pub async fn run_bench(o: &BenchOptions) -> Result<BenchReport> {
    if o.meetings.is_empty() || o.sessions.is_empty() {
        bail!("bench needs at least one meeting and one session count");
    }
    if o.speed <= 0.0 {
        bail!("--speed must be positive");
    }
    let redis = warptalk_bus::connect(&o.redis_url)
        .await
        .map_err(|e| anyhow!("cannot reach Redis (set REDIS_URL): {e}"))?;
    let cfg = crate::harness::suggestion_config(o.suggestion_overrides.as_ref())?;
    let mut levels = Vec::new();
    for &n in &o.sessions {
        eprintln!("bench: {n} concurrent session(s)");
        levels.push(run_level(&redis, o, &cfg, n).await?);
    }
    let sim = o.provider.is_fake();
    Ok(BenchReport {
        schema_version: BENCH_SCHEMA_VERSION,
        created_at: Utc::now().to_rfc3339(),
        provider: o.provider.name().into(),
        model: o.model.clone(),
        latency_simulated: sim,
        decide_ms: sim.then_some(o.decide),
        generate_ms: sim.then_some(o.generate),
        speed: o.speed,
        stagger_ms: o.stagger_ms,
        max_pipelines: o.max_pipelines,
        seed: o.seed,
        meetings: o.meetings.iter().map(|m| m.id.clone()).collect(),
        levels,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn dist_parse_and_percentiles() {
        let d = Dist::parse("300:100").unwrap();
        assert_eq!((d.mean_ms, d.sd_ms), (300.0, 100.0));
        assert_eq!(Dist::parse("50").unwrap().sd_ms, 0.0);
        assert!(Dist::parse("x").is_err());
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let p = pcts(&v);
        assert_eq!(
            (p.p50_ms, p.p95_ms, p.p99_ms, p.max_ms),
            (50.0, 95.0, 99.0, 100.0)
        );
    }

    #[test]
    fn rng_is_deterministic_and_centered() {
        let d = Dist {
            mean_ms: 700.0,
            sd_ms: 200.0,
        };
        let mut a = Rng(7);
        let mut b = Rng(7);
        let xs: Vec<f64> = (0..2000).map(|_| a.normal(d)).collect();
        assert_eq!(xs[0], b.normal(d));
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        assert!((mean - 700.0).abs() < 30.0, "{mean}");
    }
}
