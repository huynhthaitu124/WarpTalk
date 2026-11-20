//! Tracing, Prometheus metrics, and stage latency histograms.
//!
//! Decoupled from `warptalk-config`: callers pass plain parameters.

use std::net::SocketAddr;
use std::time::Instant;

use metrics::{counter, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use thiserror::Error;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Metric name for per-stage latency (kept identical to the original system).
pub const STAGE_LATENCY_METRIC: &str = "warptalk_stage_latency_ms";
/// Metric name for per-stage outcome counts.
pub const STAGE_OUTCOME_METRIC: &str = "warptalk_stage_messages_total";

const STAGE_LATENCY_BUCKETS_MS: &[f64] = &[
    250.0, 500.0, 1000.0, 2000.0, 3000.0, 5000.0, 8000.0, 12000.0, 20000.0,
];

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("invalid log filter: {0}")]
    Filter(#[from] tracing_subscriber::filter::ParseError),
    #[error("metrics exporter setup failed: {0}")]
    Metrics(#[from] metrics_exporter_prometheus::BuildError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    Timeout,
    VendorError,
    DeadLetter,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Timeout => "timeout",
            Outcome::VendorError => "vendor_error",
            Outcome::DeadLetter => "dead_letter",
        }
    }
}

/// Installs a global tracing subscriber. Safe to call more than once: if a
/// subscriber is already installed, the second call is a no-op returning `Ok`.
pub fn init_tracing(service: &str, format: LogFormat, filter: &str) -> Result<(), TelemetryError> {
    let env_filter = EnvFilter::try_new(filter)?;
    // `try_init` fails only when a global subscriber already exists; that is
    // the idempotent case, so the error is intentionally ignored.
    let _ = match format {
        LogFormat::Pretty => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer())
            .try_init(),
        LogFormat::Json => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json())
            .try_init(),
    };
    tracing::debug!(service, ?format, "tracing initialized");
    Ok(())
}

/// Installs the global Prometheus recorder with an HTTP listener on `bind`.
/// Does nothing when `bind` is `None`.
pub fn init_metrics(bind: Option<SocketAddr>) -> Result<(), TelemetryError> {
    let Some(addr) = bind else {
        return Ok(());
    };
    PrometheusBuilder::new()
        .with_http_listener(addr)
        .set_buckets_for_metric(
            Matcher::Full(STAGE_LATENCY_METRIC.to_string()),
            STAGE_LATENCY_BUCKETS_MS,
        )?
        .install()?;
    Ok(())
}

pub fn record_stage_latency(stage: &'static str, ms: f64) {
    histogram!(STAGE_LATENCY_METRIC, "stage" => stage).record(ms);
}

pub fn record_stage_outcome(stage: &'static str, outcome: Outcome) {
    counter!(STAGE_OUTCOME_METRIC, "stage" => stage, "outcome" => outcome.as_str()).increment(1);
}

/// Measures the wall-clock duration of a pipeline stage.
#[derive(Debug)]
pub struct StageTimer {
    stage: &'static str,
    start: Instant,
}

impl StageTimer {
    pub fn start(stage: &'static str) -> Self {
        Self {
            stage,
            start: Instant::now(),
        }
    }

    /// Records the elapsed latency for this stage and returns it in milliseconds.
    pub fn finish(self) -> f64 {
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        record_stage_latency(self.stage, ms);
        ms
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn outcome_strings() {
        assert_eq!(Outcome::Ok.as_str(), "ok");
        assert_eq!(Outcome::Error.as_str(), "error");
        assert_eq!(Outcome::Timeout.as_str(), "timeout");
        assert_eq!(Outcome::VendorError.as_str(), "vendor_error");
        assert_eq!(Outcome::DeadLetter.as_str(), "dead_letter");
    }

    #[test]
    fn stage_timer_returns_non_negative_ms() {
        let ms = StageTimer::start("test_stage").finish();
        assert!(ms >= 0.0);
        assert!(ms.is_finite());
    }

    #[test]
    fn init_tracing_twice_does_not_error() {
        init_tracing("test", LogFormat::Pretty, "info").unwrap();
        init_tracing("test", LogFormat::Json, "info").unwrap();
    }

    #[test]
    fn init_tracing_rejects_invalid_filter() {
        assert!(init_tracing("test", LogFormat::Pretty, "info=[bad").is_err());
    }

    #[test]
    fn init_metrics_none_is_noop() {
        init_metrics(None).unwrap();
    }
}
