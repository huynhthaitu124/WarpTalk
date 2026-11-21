//! Offline retrieval evaluation: recall@k and MRR over a labelled JSON fixture.
//!
//! Fixture format:
//! ```json
//! {"cases": [{"query": "budget approval", "relevant": ["doc-1", "doc-7"]}]}
//! ```
//! `relevant` holds ids in whatever space the ranking uses (source ids by default).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    pub query: String,
    pub relevant: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalFixture {
    pub cases: Vec<EvalCase>,
}

impl EvalFixture {
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("invalid eval fixture: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseReport {
    pub query: String,
    pub recall_at_k: f32,
    /// Reciprocal rank of the first relevant hit within the whole ranking (0 when absent).
    pub reciprocal_rank: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalReport {
    pub k: usize,
    pub cases: usize,
    pub recall_at_k: f32,
    pub mrr: f32,
    pub per_case: Vec<CaseReport>,
}

/// Score `rankings[i]` (best first, duplicates ignored) against `cases[i]`.
/// Cases without relevant ids are skipped.
pub fn evaluate(cases: &[EvalCase], rankings: &[Vec<String>], k: usize) -> EvalReport {
    let mut per_case = Vec::new();
    for (case, ranking) in cases.iter().zip(rankings) {
        if case.relevant.is_empty() {
            continue;
        }
        let mut seen: Vec<&String> = Vec::new();
        for id in ranking {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        let hits_at_k = case
            .relevant
            .iter()
            .filter(|r| seen.iter().take(k).any(|s| s == r))
            .count();
        let first = seen
            .iter()
            .position(|s| case.relevant.contains(s))
            .map_or(0.0, |p| 1.0 / (p as f32 + 1.0));
        per_case.push(CaseReport {
            query: case.query.clone(),
            recall_at_k: hits_at_k as f32 / case.relevant.len() as f32,
            reciprocal_rank: first,
        });
    }
    let n = per_case.len().max(1) as f32;
    EvalReport {
        k,
        cases: per_case.len(),
        recall_at_k: per_case.iter().map(|c| c.recall_at_k).sum::<f32>() / n,
        mrr: per_case.iter().map(|c| c.reciprocal_rank).sum::<f32>() / n,
        per_case,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn recall_and_mrr_from_fixture() {
        let fx =
            EvalFixture::from_json(include_str!("../../tests/fixtures/eval_fixture.json")).unwrap();
        assert_eq!(fx.cases.len(), 3);
        let rankings = vec![
            vec!["a".into(), "x".into(), "b".into()], // relevant a,b -> recall@2 = .5, rr 1
            vec!["x".into(), "y".into(), "c".into()], // relevant c -> recall@2 = 0, rr 1/3
            vec!["x".into(), "y".into()],             // relevant d -> 0, rr 0
        ];
        let r = evaluate(&fx.cases, &rankings, 2);
        assert_eq!(r.cases, 3);
        assert!((r.recall_at_k - 0.5 / 3.0).abs() < 1e-6, "{r:?}");
        assert!((r.mrr - (1.0 + 1.0 / 3.0) / 3.0).abs() < 1e-6, "{r:?}");
        let r3 = evaluate(&fx.cases, &rankings, 3);
        assert!((r3.per_case[0].recall_at_k - 1.0).abs() < 1e-6);
        assert!((r3.per_case[1].recall_at_k - 1.0).abs() < 1e-6);
    }

    #[test]
    fn duplicates_do_not_consume_rank_and_empty_cases_skipped() {
        let cases = vec![
            EvalCase {
                query: "q".into(),
                relevant: vec!["b".into()],
            },
            EvalCase {
                query: "none".into(),
                relevant: vec![],
            },
        ];
        let r = evaluate(
            &cases,
            &[vec!["a".into(), "a".into(), "b".into()], vec![]],
            2,
        );
        assert_eq!(r.cases, 1);
        assert_eq!(r.recall_at_k, 1.0);
        assert_eq!(r.mrr, 0.5);
        assert!(EvalFixture::from_json("{").is_err());
    }
}
