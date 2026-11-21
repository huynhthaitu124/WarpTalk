//! Search-quality metrics: per query (returned in `SearchResult`) and aggregated here.

use std::sync::Mutex;

use serde::Serialize;

use super::model::SearchMetrics;

#[derive(Debug, Default)]
struct Totals {
    queries: u64,
    zero_results: u64,
    latency_ms_sum: u64,
    latency_ms_max: u64,
    candidates_sum: u64,
    kept_sum: u64,
    returned_sum: u64,
    top_score_sum: f64,
    top_score_n: u64,
    delta_sum: f64,
    rank_changes_sum: u64,
    reranker_fallbacks: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub queries: u64,
    pub zero_results: u64,
    pub zero_result_rate: f64,
    pub avg_latency_ms: f64,
    pub max_latency_ms: u64,
    pub avg_candidates: f64,
    pub avg_kept_after_min_score: f64,
    pub avg_returned: f64,
    pub avg_top_score: f64,
    pub avg_reranker_score_delta: f64,
    pub reranker_rank_changes: u64,
    pub reranker_fallbacks: u64,
}

/// In-process aggregate. Every query is also emitted through the `metrics` facade
/// (`knowledge_search_*`), so a Prometheus exporter picks it up when installed.
#[derive(Default)]
pub struct MetricsRegistry {
    inner: Mutex<Totals>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, m: &SearchMetrics) {
        {
            let mut t = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            t.queries += 1;
            t.zero_results += u64::from(m.zero_result);
            t.latency_ms_sum += m.latency_ms;
            t.latency_ms_max = t.latency_ms_max.max(m.latency_ms);
            t.candidates_sum += m.candidates as u64;
            t.kept_sum += m.kept_after_min_score as u64;
            t.returned_sum += m.returned as u64;
            if let Some(s) = m.top_score {
                t.top_score_sum += f64::from(s);
                t.top_score_n += 1;
            }
            t.delta_sum += f64::from(m.reranker_score_delta);
            t.rank_changes_sum += m.reranker_rank_changes as u64;
            t.reranker_fallbacks += u64::from(m.reranker.ends_with("_fallback"));
        }
        metrics::counter!("knowledge_search_total").increment(1);
        if m.zero_result {
            metrics::counter!("knowledge_search_zero_results_total").increment(1);
        }
        metrics::histogram!("knowledge_search_latency_ms").record(m.latency_ms as f64);
        metrics::histogram!("knowledge_search_candidates").record(m.candidates as f64);
        metrics::histogram!("knowledge_search_kept_after_min_score")
            .record(m.kept_after_min_score as f64);
        if let Some(s) = m.top_score {
            metrics::histogram!("knowledge_search_top_score").record(f64::from(s));
        }
        metrics::histogram!("knowledge_search_reranker_score_delta")
            .record(f64::from(m.reranker_score_delta));
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let t = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let q = t.queries.max(1) as f64;
        MetricsSnapshot {
            queries: t.queries,
            zero_results: t.zero_results,
            zero_result_rate: if t.queries == 0 {
                0.0
            } else {
                t.zero_results as f64 / t.queries as f64
            },
            avg_latency_ms: t.latency_ms_sum as f64 / q,
            max_latency_ms: t.latency_ms_max,
            avg_candidates: t.candidates_sum as f64 / q,
            avg_kept_after_min_score: t.kept_sum as f64 / q,
            avg_returned: t.returned_sum as f64 / q,
            avg_top_score: if t.top_score_n == 0 {
                0.0
            } else {
                t.top_score_sum / t.top_score_n as f64
            },
            avg_reranker_score_delta: t.delta_sum / q,
            reranker_rank_changes: t.rank_changes_sum,
            reranker_fallbacks: t.reranker_fallbacks,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_rates_and_averages() {
        let r = MetricsRegistry::new();
        r.record(&SearchMetrics {
            latency_ms: 10,
            candidates: 20,
            kept_after_min_score: 4,
            returned: 3,
            top_score: Some(0.8),
            reranker_score_delta: 0.1,
            reranker_rank_changes: 2,
            reranker: "lexical".into(),
            ..SearchMetrics::default()
        });
        r.record(&SearchMetrics {
            latency_ms: 30,
            zero_result: true,
            reranker: "llm_fallback".into(),
            ..SearchMetrics::default()
        });
        let s = r.snapshot();
        assert_eq!(s.queries, 2);
        assert_eq!(s.zero_result_rate, 0.5);
        assert_eq!(s.avg_latency_ms, 20.0);
        assert_eq!(s.max_latency_ms, 30);
        assert_eq!(s.avg_candidates, 10.0);
        assert!((s.avg_top_score - 0.8).abs() < 1e-6);
        assert_eq!(s.reranker_rank_changes, 2);
        assert_eq!(s.reranker_fallbacks, 1);
    }
}
