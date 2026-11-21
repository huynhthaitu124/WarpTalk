//! Latency harness: N simulated rooms x M speakers (real-time paced synthetic speech) through
//! the real room workers, VAD/endpointer and `RoomPipeline`, with `FakeStt` standing in for the
//! provider. Stage latencies are read back from the `metrics` histograms the pipeline records
//! (`warptalk_stage_latency_ms`), so the numbers are exactly what production exports.
//!
//! The defaults reproduce the measured figures of spec 08: STT commit -> completed p50 774 ms
//! (`FakeStt` takes one fixed delay per speaker, drawn from a log-normal with the configured
//! p50/p95, so spread comes from speaker to speaker rather than from turn to turn).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use chrono::Utc;
use metrics::{
    Counter, CounterFn, Gauge, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use tokio::sync::watch;
use uuid::Uuid;
use warptalk_contracts::events::{AudioRoute, AudioRoutesSnapshot, RouteStatus};
use warptalk_kernel::{ParticipantId, RoomId};
use warptalk_pipeline_runtime::testing::{FakeStt, FakeSttConfig, energy_vad_factory};
use warptalk_pipeline_runtime::{
    DROPPED_FRAMES_METRIC, PipelineConfig, STAGE_ENDPOINT_DECISION, STAGE_STT_FINAL,
    STAGE_STT_FIRST_TEXT,
};
use warptalk_telemetry::STAGE_LATENCY_METRIC;

use crate::downstream::{
    CAPTION_TOPIC, STAGE_E2E_FINAL_CAPTION, STAGE_E2E_FIRST_CAPTION, STAGE_RESULT_PUBLISH,
    SttOnlyStage,
};
use crate::results::MemoryResultSink;
use crate::routes::RoutesView;
use crate::simulated::{SimParticipant, SimulatedTransport};
use crate::transport::MEDIA_EVENTS_DROPPED_METRIC;
use crate::worker::{RoomWorker, RoomWorkerDeps};

#[derive(Debug, Clone, Serialize)]
pub struct BenchConfig {
    pub rooms: usize,
    pub speakers: usize,
    pub seconds: u64,
    pub seed: u64,
    /// STT commit -> completed, median (spec 08: 774 ms).
    pub stt_p50_ms: f64,
    pub stt_p95_ms: f64,
    /// Delay from commit to the early (first) segment.
    pub stt_early_ms: u64,
    pub speech_ms: u32,
    pub pause_ms: u32,
    /// Delivery format of the simulated transport (LiveKit native: 48 kHz).
    pub sample_rate_hz: u32,
    pub frame_ms: u32,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            rooms: 2,
            speakers: 2,
            seconds: 10,
            seed: 42,
            stt_p50_ms: 774.0,
            stt_p95_ms: 1_300.0,
            stt_early_ms: 400,
            speech_ms: 3_000,
            pause_ms: 2_000,
            sample_rate_hz: 48_000,
            frame_ms: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Percentiles {
    pub count: usize,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Percentiles {
    pub fn from_samples(samples: &[f64]) -> Self {
        let mut v: Vec<f64> = samples.iter().copied().filter(|x| x.is_finite()).collect();
        v.sort_by(f64::total_cmp);
        let pick = |q: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            // nearest-rank
            let rank = (q * v.len() as f64).ceil() as usize;
            v[rank.clamp(1, v.len()) - 1]
        };
        Self {
            count: v.len(),
            mean: if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 },
            p50: pick(0.50),
            p95: pick(0.95),
            p99: pick(0.99),
            max: v.last().copied().unwrap_or(0.0),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub config: BenchConfig,
    /// Wall time from first join to last worker drained.
    pub wall_secs: f64,
    /// Process CPU time (user + system) consumed during the run; `None` where unavailable.
    pub cpu_secs: Option<f64>,
    /// `cpu_secs / wall_secs * 100` (100 = one full core).
    pub cpu_percent_of_one_core: Option<f64>,
    pub audio_seconds_total: f64,
    pub frames_dropped: u64,
    pub media_events_dropped: u64,
    pub stt_results_published: usize,
    pub captions_published: usize,
    /// Stage name -> latency distribution in ms.
    pub stages: BTreeMap<String, Percentiles>,
}

const STAGES: [&str; 6] = [
    STAGE_ENDPOINT_DECISION,
    STAGE_STT_FIRST_TEXT,
    STAGE_STT_FINAL,
    STAGE_RESULT_PUBLISH,
    STAGE_E2E_FIRST_CAPTION,
    STAGE_E2E_FINAL_CAPTION,
];

impl BenchReport {
    pub fn render_table(&self) -> String {
        let c = &self.config;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "pipeline-bench: {} rooms x {} speakers, {} s, seed {}",
            c.rooms, c.speakers, c.seconds, c.seed
        );
        let _ = writeln!(
            s,
            "fake STT: commit->completed p50 {:.0} ms / p95 {:.0} ms, first text {} ms; speech {} ms, pause {} ms; {} Hz / {} ms blocks",
            c.stt_p50_ms, c.stt_p95_ms, c.stt_early_ms, c.speech_ms, c.pause_ms, c.sample_rate_hz, c.frame_ms
        );
        let _ = writeln!(
            s,
            "\n{:<26}{:>7}{:>10}{:>10}{:>10}{:>10}",
            "stage (ms)", "n", "p50", "p95", "p99", "max"
        );
        for (name, p) in &self.stages {
            let _ = writeln!(
                s,
                "{:<26}{:>7}{:>10.1}{:>10.1}{:>10.1}{:>10.1}",
                name, p.count, p.p50, p.p95, p.p99, p.max
            );
        }
        let _ = writeln!(s);
        let _ = writeln!(s, "frames dropped (speaker queues)  {}", self.frames_dropped);
        let _ = writeln!(s, "media events dropped             {}", self.media_events_dropped);
        let _ = writeln!(s, "stt:results entries              {}", self.stt_results_published);
        let _ = writeln!(s, "captions published               {}", self.captions_published);
        let _ = writeln!(s, "audio processed                  {:.0} s", self.audio_seconds_total);
        let _ = writeln!(s, "wall                             {:.2} s", self.wall_secs);
        match (self.cpu_secs, self.cpu_percent_of_one_core) {
            (Some(cpu), Some(pct)) => {
                let _ = writeln!(s, "cpu time                         {cpu:.2} s ({pct:.1}% of one core)");
            }
            _ => {
                let _ = writeln!(s, "cpu time                         n/a");
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------------------------
// Metrics capture
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct CounterSink(AtomicU64);

impl CounterFn for CounterSink {
    fn increment(&self, value: u64) {
        self.0.fetch_add(value, Ordering::Relaxed);
    }

    fn absolute(&self, value: u64) {
        self.0.fetch_max(value, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct HistSink(Mutex<Vec<f64>>);

impl HistogramFn for HistSink {
    fn record(&self, value: f64) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(value);
    }
}

/// In-memory `metrics` recorder. Histograms of `warptalk_stage_latency_ms` are keyed by their
/// `stage` label, everything else by metric name.
#[derive(Default)]
pub struct Capture {
    hists: Mutex<HashMap<String, Arc<HistSink>>>,
    counters: Mutex<HashMap<String, Arc<CounterSink>>>,
}

impl Capture {
    pub fn histogram(&self, name: &str) -> Vec<f64> {
        self.hists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .map(|h| h.0.lock().unwrap_or_else(PoisonError::into_inner).clone())
            .unwrap_or_default()
    }

    pub fn counter(&self, name: &str) -> u64 {
        self.counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .map_or(0, |c| c.0.load(Ordering::Relaxed))
    }

    /// Forgets samples recorded so far (handles already handed out keep working).
    pub fn reset(&self) {
        for h in self.hists.lock().unwrap_or_else(PoisonError::into_inner).values() {
            h.0.lock().unwrap_or_else(PoisonError::into_inner).clear();
        }
        for c in self.counters.lock().unwrap_or_else(PoisonError::into_inner).values() {
            c.0.store(0, Ordering::Relaxed);
        }
    }
}

struct CaptureRecorder(Arc<Capture>);

impl Recorder for CaptureRecorder {
    fn describe_counter(&self, _: metrics::KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        let sink = self
            .0
            .counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.name().to_owned())
            .or_default()
            .clone();
        Counter::from_arc(sink)
    }

    fn register_gauge(&self, _: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::noop()
    }

    fn register_histogram(&self, key: &Key, _: &Metadata<'_>) -> Histogram {
        let name = if key.name() == STAGE_LATENCY_METRIC {
            key.labels()
                .find(|l| l.key() == "stage")
                .map_or_else(|| key.name().to_owned(), |l| l.value().to_owned())
        } else {
            key.name().to_owned()
        };
        let sink = self
            .0
            .hists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(name)
            .or_default()
            .clone();
        Histogram::from_arc(sink)
    }
}

static CAPTURE: OnceLock<Arc<Capture>> = OnceLock::new();

/// Installs the capturing recorder as the process-global `metrics` recorder (once). The
/// benchmark owns the process, so this must not be combined with the Prometheus exporter.
pub fn install_capture() -> Arc<Capture> {
    CAPTURE
        .get_or_init(|| {
            let c = Arc::new(Capture::default());
            // Fails only if another recorder was installed first; the capture then stays empty.
            let _ = metrics::set_global_recorder(CaptureRecorder(c.clone()));
            c
        })
        .clone()
}

/// User + system CPU seconds of this process (Linux `/proc`, 100 ticks/s).
pub fn process_cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

// ---------------------------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------------------------

/// Standard normal via Box-Muller.
fn gaussian(rng: &mut StdRng) -> f64 {
    let u1: f64 = rng.gen_range(f64::EPSILON..1.0);
    let u2: f64 = rng.gen_range(0.0..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Log-normal sample with the given median and 95th percentile.
fn lognormal(rng: &mut StdRng, p50: f64, p95: f64) -> f64 {
    let sigma = (p95.max(p50) / p50).ln() / 1.645;
    p50 * (sigma * gaussian(rng)).exp()
}

pub async fn run_bench(cfg: BenchConfig) -> BenchReport {
    let capture = install_capture();
    capture.reset();
    let mut rng = StdRng::seed_from_u64(cfg.seed);

    // Rooms, speakers, routes (en -> vi for every speaker) and per-speaker STT delay.
    let mut plans: HashMap<RoomId, Vec<SimParticipant>> = HashMap::new();
    let mut views: Vec<(RoomId, RoutesView)> = Vec::new();
    let mut stt_cfg = FakeSttConfig {
        early_delay: Duration::from_millis(cfg.stt_early_ms),
        default_delay: Duration::from_millis(cfg.stt_p50_ms as u64),
        ..FakeSttConfig::default()
    };
    for _ in 0..cfg.rooms {
        let room = RoomId::new();
        let mut participants = Vec::new();
        let mut routes = Vec::new();
        for _ in 0..cfg.speakers {
            let pid = ParticipantId::new();
            let jitter = |rng: &mut StdRng, ms: u32| (f64::from(ms) * rng.gen_range(0.8..1.2)) as u32;
            let speech = jitter(&mut rng, cfg.speech_ms);
            let pause = jitter(&mut rng, cfg.pause_ms);
            let offset = Duration::from_millis(rng.gen_range(0..u64::from(cfg.pause_ms.max(1))));
            participants.push(
                SimParticipant::synthetic(pid.to_string(), speech, pause)
                    .format(cfg.sample_rate_hz, cfg.frame_ms)
                    .join_after(offset),
            );
            let final_ms = lognormal(&mut rng, cfg.stt_p50_ms, cfg.stt_p95_ms);
            let after_first = (final_ms - cfg.stt_early_ms as f64).max(0.0) as u64;
            stt_cfg
                .delay_by_speaker
                .insert(pid.to_string(), Duration::from_millis(after_first));
            routes.push(AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: pid,
                target_participant_id: ParticipantId::new(),
                source_language: "en".into(),
                target_language: "vi".into(),
                voice_clone_enabled: false,
                status: RouteStatus::Broadcasting,
            });
        }
        let snapshot = AudioRoutesSnapshot {
            generated_at: Utc::now(),
            room_status: "Active".into(),
            translation_active: true,
            room_languages: vec!["en".into(), "vi".into()],
            routes,
        };
        views.push((room, RoutesView::from_snapshot(&snapshot)));
        plans.insert(room, participants);
    }

    let transport = Arc::new(SimulatedTransport::new(Arc::new(move |room| {
        plans.get(room).cloned().unwrap_or_default()
    })));
    let stt = Arc::new(FakeStt::new(stt_cfg));
    let results = Arc::new(MemoryResultSink::default());
    let (_cfg_tx, cfg_rx) = watch::channel(PipelineConfig::default());

    let cpu0 = process_cpu_seconds();
    let t0 = Instant::now();
    let mut workers = Vec::new();
    for (room, view) in views {
        let started = RoomWorker::start(RoomWorkerDeps {
            room,
            transport: transport.clone(),
            stt: stt.clone(),
            vad_factory: energy_vad_factory(),
            downstream: Arc::new(SttOnlyStage),
            results: results.clone(),
            config: cfg_rx.clone(),
            flash_override: None,
            identity: "pipeline-bench".into(),
            initial_routes: view,
        })
        .await;
        match started {
            Ok(w) => workers.push(w),
            Err(e) => tracing::error!(%room, error = %e, "bench room failed to start"),
        }
    }
    tokio::time::sleep(Duration::from_secs(cfg.seconds)).await;
    futures::future::join_all(workers.iter().map(RoomWorker::stop)).await;
    let wall = t0.elapsed().as_secs_f64();
    let cpu = match (cpu0, process_cpu_seconds()) {
        (Some(a), Some(b)) => Some((b - a).max(0.0)),
        _ => None,
    };

    let captions_published = workers
        .iter()
        .filter_map(|w| transport.capture(w.room()))
        .map(|c| c.json_on(CAPTION_TOPIC).len())
        .sum();
    BenchReport {
        wall_secs: wall,
        cpu_secs: cpu,
        cpu_percent_of_one_core: cpu.map(|c| c / wall * 100.0),
        audio_seconds_total: (cfg.rooms * cfg.speakers) as f64 * cfg.seconds as f64,
        frames_dropped: capture.counter(DROPPED_FRAMES_METRIC),
        media_events_dropped: capture.counter(MEDIA_EVENTS_DROPPED_METRIC),
        stt_results_published: results.len(),
        captions_published,
        stages: STAGES
            .iter()
            .map(|s| ((*s).to_owned(), Percentiles::from_samples(&capture.histogram(s))))
            .collect(),
        config: cfg,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let p = Percentiles::from_samples(&v);
        assert_eq!(p.count, 100);
        assert_eq!(p.p50, 50.0);
        assert_eq!(p.p95, 95.0);
        assert_eq!(p.p99, 99.0);
        assert_eq!(p.max, 100.0);
        assert_eq!(Percentiles::from_samples(&[]).p50, 0.0);
    }

    #[test]
    fn lognormal_median_matches() {
        let mut rng = StdRng::seed_from_u64(1);
        let mut xs: Vec<f64> = (0..20_000).map(|_| lognormal(&mut rng, 774.0, 1300.0)).collect();
        xs.sort_by(f64::total_cmp);
        let med = xs[xs.len() / 2];
        let p95 = xs[xs.len() * 95 / 100];
        assert!((med - 774.0).abs() < 25.0, "median {med}");
        assert!((p95 - 1300.0).abs() < 80.0, "p95 {p95}");
    }
}
