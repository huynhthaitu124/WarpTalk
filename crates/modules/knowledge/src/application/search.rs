//! Query path: embed -> vector search (filtered, scoped) -> defense-in-depth check ->
//! `min_score` -> re-rank -> MMR diversity -> citations, with per-query metrics.

use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult};
use warptalk_llm::{EmbeddingProvider, LlmProvider};
use warptalk_qdrant::{SearchRequest, VectorStore};
use warptalk_settings::{NS_RAG, ScopeChain, SettingsStore};

use super::metrics::MetricsRegistry;
use super::model::{AccessContext, SearchFilters, SearchHit, SearchMetrics, SearchResult};
use super::ports::KnowledgeRepository;
use super::query::search_filter;
use super::rerank::{LexicalReranker, LlmReranker, RerankInput, RerankWeights, Reranker};
use crate::domain::rerank::{MmrItem, mmr_select, token_set};
use crate::domain::{ChunkPayload, RagConfig, RerankMode};

pub struct SearchDeps {
    pub repo: Arc<dyn KnowledgeRepository>,
    pub vectors: Arc<dyn VectorStore>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    pub settings: SettingsStore,
    pub llm: Option<Arc<dyn LlmProvider>>,
    pub metrics: Arc<MetricsRegistry>,
}

pub struct KnowledgeService {
    d: SearchDeps,
}

struct Candidate {
    id: Uuid,
    dense: f32,
    payload: ChunkPayload,
    text: String,
}

fn rank_changes(dense_order: &[usize], final_order: &[usize]) -> usize {
    dense_order
        .iter()
        .zip(final_order)
        .filter(|(a, b)| a != b)
        .count()
}

fn order_by(scores: &[f32]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|a, b| {
        scores[*b]
            .partial_cmp(&scores[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    idx
}

impl KnowledgeService {
    pub fn new(d: SearchDeps) -> Self {
        Self { d }
    }

    pub fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.d.metrics
    }

    pub(crate) async fn config(&self, ctx: &AccessContext) -> AppResult<RagConfig> {
        self.d
            .settings
            .get::<RagConfig>(NS_RAG, ScopeChain::workspace(ctx.workspace))
            .await
    }

    pub async fn search(
        &self,
        ctx: &AccessContext,
        query: &str,
        filters: &SearchFilters,
    ) -> AppResult<SearchResult> {
        let started = Instant::now();
        let cfg = self.config(ctx).await?;
        let mut m = SearchMetrics {
            reranker: "off".into(),
            ..SearchMetrics::default()
        };
        let finish = |mut m: SearchMetrics, chunks: Vec<SearchHit>, started: Instant| {
            m.latency_ms = started.elapsed().as_millis() as u64;
            m.returned = chunks.len();
            m.top_score = chunks.first().map(|h| h.score);
            m.zero_result = chunks.is_empty();
            self.d.metrics.record(&m);
            tracing::debug!(
                latency_ms = m.latency_ms,
                candidates = m.candidates,
                kept = m.kept_after_min_score,
                returned = m.returned,
                "knowledge search"
            );
            SearchResult { chunks, metrics: m }
        };

        let query = query.trim();
        if query.is_empty() {
            return Ok(finish(m, Vec::new(), started));
        }
        let Some(profile) = self.d.repo.active_profile(ctx.workspace.as_uuid()).await? else {
            return Ok(finish(m, Vec::new(), started));
        };

        // Embed with the ACTIVE profile's model (a pending profile switch must not change it).
        let t_embed = Instant::now();
        let timeout = Duration::from_secs_f32(cfg.base.search_timeout_seconds);
        let embedded = tokio::time::timeout(
            timeout,
            self.d
                .embeddings
                .embed(&profile.profile.model, &[query.to_owned()]),
        )
        .await
        .map_err(|_| AppError::unavailable("embedding timed out"))?
        .map_err(|e| {
            tracing::warn!(error = %e, "query embedding failed");
            AppError::unavailable("embedding provider unavailable")
        })?;
        m.embed_ms = t_embed.elapsed().as_millis() as u64;
        let vector = embedded
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| AppError::unavailable("embedding provider returned no vector"))?;
        if vector.len() != profile.profile.dims {
            return Err(AppError::unavailable(
                "embedding dimensions differ from the active index profile; reindex required",
            ));
        }

        let top_k = filters.limit.map_or(cfg.base.top_k as usize, |l| l.clamp(1, 50));
        let candidate_k = (cfg.ext.candidate_k as usize).max(top_k);
        let scope = ctx.effective_scope();
        let filter = search_filter(ctx, &scope, filters);
        let hits = self
            .d
            .vectors
            .search(
                &profile.collection,
                &SearchRequest::new(vector, candidate_k).with_filter(filter),
            )
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "vector search failed");
                AppError::unavailable("vector search unavailable")
            })?;
        m.candidates = hits.len();

        // Defense in depth: never trust the store's filter alone.
        let mut cands: Vec<Candidate> = Vec::new();
        for h in hits {
            let Ok(payload) = ChunkPayload::from_value(&h.payload) else {
                tracing::warn!(point = %h.id, "dropping point with unreadable payload");
                continue;
            };
            let visible = payload.workspace_id == ctx.workspace.as_uuid()
                && payload.ai_retrieval
                && payload.retention_state == "active"
                && payload.deletion_state == "active"
                && scope.admits(&payload)
                && (filters.source_types.is_empty()
                    || filters.source_types.contains(&payload.source_type));
            if !visible || h.score < cfg.ext.min_score {
                continue;
            }
            cands.push(Candidate {
                id: Uuid::parse_str(&h.id).unwrap_or(payload.chunk_id),
                dense: h.score,
                text: payload.searchable_text(),
                payload,
            });
        }
        cands.sort_by(|a, b| {
            b.dense
                .partial_cmp(&a.dense)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        m.kept_after_min_score = cands.len();
        if cands.is_empty() {
            return Ok(finish(m, Vec::new(), started));
        }

        // Re-rank.
        let dense: Vec<f32> = cands.iter().map(|c| c.dense).collect();
        let weights = RerankWeights {
            dense: cfg.ext.rerank_dense_weight,
            other: cfg.ext.rerank_weight,
        };
        let inputs: Vec<RerankInput<'_>> = cands
            .iter()
            .map(|c| RerankInput {
                text: &c.text,
                dense: c.dense,
            })
            .collect();
        let (final_scores, used) = match cfg.ext.rerank_mode {
            RerankMode::Off => (dense.clone(), "off".to_owned()),
            RerankMode::Lexical => (
                LexicalReranker
                    .rerank(query, &inputs, weights)
                    .await
                    .unwrap_or_else(|_| dense.clone()),
                "lexical".to_owned(),
            ),
            RerankMode::Llm => {
                let llm_result = match &self.d.llm {
                    Some(llm) if !cfg.ext.rerank_model.is_empty() => {
                        LlmReranker::new(
                            llm.clone(),
                            cfg.ext.rerank_model.clone(),
                            cfg.ext.rerank_llm_candidates as usize,
                        )
                        .rerank(query, &inputs, weights)
                        .await
                    }
                    _ => Err(super::rerank::RerankError(
                        "llm reranker not configured".into(),
                    )),
                };
                match llm_result {
                    Ok(s) => (s, "llm".to_owned()),
                    Err(e) => {
                        tracing::warn!(error = %e, "llm rerank failed; using lexical");
                        (
                            LexicalReranker
                                .rerank(query, &inputs, weights)
                                .await
                                .unwrap_or_else(|_| dense.clone()),
                            "lexical_fallback".to_owned(),
                        )
                    }
                }
            }
        };
        m.reranker = used;
        let dense_order = order_by(&dense);
        let final_order = order_by(&final_scores);
        m.reranker_rank_changes = if cfg.ext.rerank_mode == RerankMode::Off {
            0
        } else {
            rank_changes(&dense_order, &final_order)
        };
        m.top_dense_score = dense_order.first().map(|i| dense[*i]);
        m.reranker_score_delta = match (final_order.first(), dense_order.first()) {
            (Some(f), Some(d)) if cfg.ext.rerank_mode != RerankMode::Off => {
                final_scores[*f] - dense[*d]
            }
            _ => 0.0,
        };

        // Diversity across documents / meetings.
        let items: Vec<MmrItem> = cands
            .iter()
            .zip(&final_scores)
            .map(|(c, s)| MmrItem {
                score: *s,
                source_key: format!("{}:{}", c.payload.source_type, c.payload.source_id),
                tokens: token_set(&c.payload.text),
            })
            .collect();
        let picked = mmr_select(
            &items,
            top_k,
            cfg.ext.mmr_lambda,
            cfg.ext.max_chunks_per_source as usize,
        );
        let mut cands: Vec<Option<Candidate>> = cands.into_iter().map(Some).collect();
        let chunks: Vec<SearchHit> = picked
            .into_iter()
            .filter_map(|i| {
                let c = cands[i].take()?;
                Some(SearchHit {
                    chunk_id: c.id,
                    score: final_scores[i],
                    dense_score: c.dense,
                    citation: c.payload.citation(),
                    payload: c.payload,
                })
            })
            .collect();
        Ok(finish(m, chunks, started))
    }
}
