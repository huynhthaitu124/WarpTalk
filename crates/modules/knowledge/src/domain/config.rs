//! `RagConfig`: the `rag` settings namespace (`warptalk_settings::NS_RAG`) as this module reads
//! it. `RagSettings` fields are kept as-is (flattened); the knobs this module added live in
//! [`RagExtSettings`] under the same namespace key, all with serde defaults.

use serde::{Deserialize, Serialize};
use warptalk_settings::{RagSettings, Validate, check_range};

use super::chunker::ChunkingParams;
use super::pii::PiiPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RerankMode {
    /// Dense order and scores unchanged.
    Off,
    /// Dense score fused with local BM25-lite.
    #[default]
    Lexical,
    /// Dense score fused with an LLM relevance score (falls back to lexical on failure).
    Llm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RagExtSettings {
    // retrieval
    /// Dense score floor (cosine) applied after the vector query.
    pub min_score: f32,
    /// Vector candidates fetched before filtering / re-ranking.
    pub candidate_k: u32,
    pub rerank_mode: RerankMode,
    pub rerank_dense_weight: f32,
    /// Weight of the second signal (lexical or LLM score).
    pub rerank_weight: f32,
    /// Model used when `rerank_mode = llm`.
    pub rerank_model: String,
    /// Candidates sent to the LLM reranker.
    pub rerank_llm_candidates: u32,
    /// MMR trade-off: 1.0 = relevance only, lower = more diversity.
    pub mmr_lambda: f32,
    /// Cap on chunks from one document/meeting in a result (0 = unlimited).
    pub max_chunks_per_source: u32,
    // chunking
    pub chunk_target_tokens: u32,
    pub chunk_max_tokens: u32,
    pub chunk_overlap_tokens: u32,
    pub transcript_window_ms: u32,
    pub transcript_gap_ms: u32,
    // PII / DLP
    pub pii_policy: PiiPolicy,
    /// Source types scanned before indexing.
    pub pii_source_types: Vec<String>,
    // facts
    pub fact_extraction_enabled: bool,
    pub fact_model: String,
    pub fact_min_confidence: f32,
    // retries
    pub retry_max_attempts: u32,
    pub retry_base_backoff_ms: u32,
    pub retry_max_backoff_ms: u32,
}

impl Default for RagExtSettings {
    fn default() -> Self {
        Self {
            min_score: 0.25,
            candidate_k: 30,
            rerank_mode: RerankMode::Lexical,
            rerank_dense_weight: 0.7,
            rerank_weight: 0.3,
            rerank_model: String::new(),
            rerank_llm_candidates: 20,
            mmr_lambda: 0.7,
            max_chunks_per_source: 3,
            chunk_target_tokens: 300,
            chunk_max_tokens: 450,
            chunk_overlap_tokens: 40,
            transcript_window_ms: 60_000,
            transcript_gap_ms: 20_000,
            pii_policy: PiiPolicy::Mask,
            pii_source_types: vec!["transcript".to_owned()],
            fact_extraction_enabled: false,
            fact_model: String::new(),
            fact_min_confidence: 0.5,
            retry_max_attempts: 5,
            retry_base_backoff_ms: 2_000,
            retry_max_backoff_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RagConfig {
    #[serde(flatten)]
    pub base: RagSettings,
    #[serde(flatten)]
    pub ext: RagExtSettings,
}

impl Validate for RagConfig {
    fn validate(&self) -> Result<(), String> {
        self.base.validate()?;
        let e = &self.ext;
        check_range("min_score", e.min_score, -1.0, 1.0)?;
        check_range("candidate_k", e.candidate_k, 1, 500)?;
        if e.candidate_k < self.base.top_k {
            return Err("candidate_k must be >= top_k".into());
        }
        check_range("rerank_dense_weight", e.rerank_dense_weight, 0.0, 1.0)?;
        check_range("rerank_weight", e.rerank_weight, 0.0, 1.0)?;
        if e.rerank_dense_weight + e.rerank_weight <= 0.0 {
            return Err("rerank weights must not both be zero".into());
        }
        check_range("rerank_llm_candidates", e.rerank_llm_candidates, 1, 100)?;
        check_range("mmr_lambda", e.mmr_lambda, 0.0, 1.0)?;
        check_range("max_chunks_per_source", e.max_chunks_per_source, 0, 100)?;
        check_range("chunk_target_tokens", e.chunk_target_tokens, 32, 4000)?;
        check_range("chunk_max_tokens", e.chunk_max_tokens, 32, 8000)?;
        if e.chunk_max_tokens < e.chunk_target_tokens {
            return Err("chunk_max_tokens must be >= chunk_target_tokens".into());
        }
        if e.chunk_overlap_tokens >= e.chunk_target_tokens {
            return Err("chunk_overlap_tokens must be < chunk_target_tokens".into());
        }
        check_range("chunk_char_limit", self.base.chunk_char_limit, 200, 20_000)?;
        check_range(
            "transcript_window_ms",
            e.transcript_window_ms,
            1_000,
            3_600_000,
        )?;
        check_range("transcript_gap_ms", e.transcript_gap_ms, 0, 3_600_000)?;
        check_range("fact_min_confidence", e.fact_min_confidence, 0.0, 1.0)?;
        check_range("retry_max_attempts", e.retry_max_attempts, 1, 20)?;
        check_range("retry_base_backoff_ms", e.retry_base_backoff_ms, 0, 600_000)?;
        check_range("retry_max_backoff_ms", e.retry_max_backoff_ms, 0, 3_600_000)?;
        for t in &e.pii_source_types {
            t.parse::<super::payload::SourceType>()?;
        }
        Ok(())
    }
}

impl RagConfig {
    pub fn chunking(&self) -> ChunkingParams {
        ChunkingParams {
            target_tokens: self.ext.chunk_target_tokens as usize,
            max_tokens: self.ext.chunk_max_tokens as usize,
            overlap_tokens: self.ext.chunk_overlap_tokens as usize,
            char_limit: self.base.chunk_char_limit as usize,
            transcript_window_ms: u64::from(self.ext.transcript_window_ms),
            transcript_gap_ms: u64::from(self.ext.transcript_gap_ms),
        }
    }

    pub fn pii_applies_to(&self, source_type: super::payload::SourceType) -> bool {
        self.ext.pii_policy != PiiPolicy::Off
            && self
                .ext
                .pii_source_types
                .iter()
                .any(|t| t == source_type.as_str())
    }

    /// Backoff before retry number `attempt` (1-based: the delay after the first failure).
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        let base = u64::from(self.ext.retry_base_backoff_ms);
        let max = u64::from(self.ext.retry_max_backoff_ms);
        base.saturating_mul(1u64 << attempt.saturating_sub(1).min(20))
            .min(max)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_validate_and_flatten_roundtrip() {
        let c = RagConfig::default();
        c.validate().unwrap();
        assert_eq!(c.base.top_k, 5);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["top_k"], 5);
        assert_eq!(v["candidate_k"], 30);
        assert_eq!(serde_json::from_value::<RagConfig>(v).unwrap(), c);
    }

    #[test]
    fn partial_documents_fill_defaults_from_both_halves() {
        let c: RagConfig = serde_json::from_value(
            json!({"top_k": 8, "min_score": 0.4, "rerank_mode": "off", "pii_policy": "block"}),
        )
        .unwrap();
        assert_eq!(c.base.top_k, 8);
        assert_eq!(c.base.embedding_dimensions, 1536);
        assert_eq!(c.ext.min_score, 0.4);
        assert_eq!(c.ext.rerank_mode, RerankMode::Off);
        assert_eq!(c.ext.pii_policy, PiiPolicy::Block);
        assert_eq!(c.ext.candidate_k, 30);
        c.validate().unwrap();
    }

    #[test]
    fn validation_rejects_bad_values() {
        let bad = |v: serde_json::Value| {
            serde_json::from_value::<RagConfig>(v)
                .unwrap()
                .validate()
                .is_err()
        };
        assert!(bad(json!({"candidate_k": 3, "top_k": 5})));
        assert!(bad(json!({"min_score": 2.0})));
        assert!(bad(
            json!({"rerank_dense_weight": 0.0, "rerank_weight": 0.0})
        ));
        assert!(bad(json!({"chunk_overlap_tokens": 300})));
        assert!(bad(json!({"pii_source_types": ["nonsense"]})));
        assert!(bad(json!({"top_k": 0})));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let c = RagConfig::default();
        assert_eq!(c.backoff_ms(1), 2_000);
        assert_eq!(c.backoff_ms(2), 4_000);
        assert_eq!(c.backoff_ms(10), 60_000);
    }
}
