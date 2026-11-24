//! A provider wrapper that records latency and token usage of every call.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, ProviderId, Request, Response, StreamEvent,
};

use crate::cost::{self, Price};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallRecord {
    /// `suggestion_decide`, `suggestion_generate`, `summary`, `eval_judge`.
    pub purpose: String,
    pub latency_ms: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub ok: bool,
}

pub struct RecordingProvider {
    inner: Arc<dyn LlmProvider>,
    log: Mutex<Vec<CallRecord>>,
    /// Replace wall-clock latency by a deterministic function of the output size (fake runs).
    simulated_latency: bool,
}

impl RecordingProvider {
    pub fn new(inner: Arc<dyn LlmProvider>, simulated_latency: bool) -> Self {
        Self {
            inner,
            log: Mutex::new(Vec::new()),
            simulated_latency,
        }
    }

    pub fn records(&self) -> Vec<CallRecord> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    fn push(&self, r: CallRecord) {
        if let Ok(mut l) = self.log.lock() {
            l.push(r);
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let purpose = req
            .metadata
            .get("purpose")
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let started = Instant::now();
        let res = self.inner.complete(req).await;
        let wall = started.elapsed().as_secs_f64() * 1000.0;
        let (ok, tin, tout) = match &res {
            Ok(r) => (
                true,
                u64::from(r.usage.total_input()),
                u64::from(r.usage.output + r.usage.reasoning),
            ),
            Err(_) => (false, 0, 0),
        };
        let latency_ms = if self.simulated_latency {
            20.0 + 2.0 * tout as f64
        } else {
            wall
        };
        self.push(CallRecord {
            purpose,
            latency_ms,
            input_tokens: tin,
            output_tokens: tout,
            ok,
        });
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencyStats {
    pub calls: u64,
    pub failed: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

pub fn latency_stats(records: &[&CallRecord]) -> LatencyStats {
    if records.is_empty() {
        return LatencyStats::default();
    }
    let mut v: Vec<f64> = records.iter().map(|r| r.latency_ms).collect();
    v.sort_by(f64::total_cmp);
    let pick = |q: f64| {
        let idx = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
        v[idx]
    };
    LatencyStats {
        calls: v.len() as u64,
        failed: records.iter().filter(|r| !r.ok).count() as u64,
        mean_ms: v.iter().sum::<f64>() / v.len() as f64,
        p50_ms: pick(0.50),
        p95_ms: pick(0.95),
        max_ms: v.last().copied().unwrap_or(0.0),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimate from the price table (or `--price-*`); absent for unknown models.
    pub cost_usd: Option<f64>,
}

pub fn usage_totals(records: &[CallRecord], price: Option<Price>) -> UsageTotals {
    let input_tokens: u64 = records.iter().map(|r| r.input_tokens).sum();
    let output_tokens: u64 = records.iter().map(|r| r.output_tokens).sum();
    UsageTotals {
        calls: records.len() as u64,
        input_tokens,
        output_tokens,
        cost_usd: cost::estimate(price, input_tokens, output_tokens),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank() {
        let recs: Vec<CallRecord> = (1..=10)
            .map(|i| CallRecord {
                purpose: "p".into(),
                latency_ms: f64::from(i) * 10.0,
                input_tokens: 1,
                output_tokens: 1,
                ok: i != 10,
            })
            .collect();
        let refs: Vec<&CallRecord> = recs.iter().collect();
        let s = latency_stats(&refs);
        assert_eq!(s.calls, 10);
        assert_eq!(s.failed, 1);
        assert_eq!(s.p50_ms, 50.0);
        assert_eq!(s.p95_ms, 100.0);
        assert_eq!(s.max_ms, 100.0);
    }
}
