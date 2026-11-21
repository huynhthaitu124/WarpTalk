//! Lexical (BM25-lite) scoring, score fusion and MMR-style diversity selection. Pure.

use std::collections::{HashMap, HashSet};

use super::text::tokenize;

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// BM25 of `query` against each doc, computed over the candidate set itself (idf from the set)
/// and normalised so a document containing every query term once at average length scores 1.0
/// (clipped to `0..=1`).
pub fn lexical_scores(query: &str, docs: &[&str]) -> Vec<f32> {
    let n = docs.len();
    if n == 0 {
        return Vec::new();
    }
    let mut q_terms = tokenize(query);
    q_terms.sort();
    q_terms.dedup();
    if q_terms.is_empty() {
        return vec![0.0; n];
    }
    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|d| tokenize(d)).collect();
    let avg_len = (doc_tokens.iter().map(Vec::len).sum::<usize>() as f32 / n as f32).max(1.0);
    let tfs: Vec<HashMap<&str, f32>> = doc_tokens
        .iter()
        .map(|toks| {
            let mut m: HashMap<&str, f32> = HashMap::new();
            for t in toks {
                *m.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
            m
        })
        .collect();
    let idf: Vec<f32> = q_terms
        .iter()
        .map(|t| {
            let df = tfs.iter().filter(|m| m.contains_key(t.as_str())).count() as f32;
            (1.0 + (n as f32 - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    let norm: f32 = idf.iter().sum();
    if norm <= 0.0 {
        return vec![0.0; n];
    }
    doc_tokens
        .iter()
        .zip(&tfs)
        .map(|(toks, tf)| {
            let dl = toks.len() as f32;
            let score: f32 = q_terms
                .iter()
                .zip(&idf)
                .map(|(t, idf)| {
                    let f = tf.get(t.as_str()).copied().unwrap_or(0.0);
                    if f == 0.0 {
                        0.0
                    } else {
                        idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * dl / avg_len))
                    }
                })
                .sum();
            (score / norm).clamp(0.0, 1.0)
        })
        .collect()
}

/// Weighted fusion of the dense (cosine) score with a second relevance signal in `0..=1`.
/// Weights are normalised; the dense score is clipped to `0..=1`.
pub fn fuse(dense: f32, other: f32, w_dense: f32, w_other: f32) -> f32 {
    let total = w_dense + w_other;
    if total <= 0.0 {
        return dense;
    }
    (w_dense * dense.clamp(0.0, 1.0) + w_other * other.clamp(0.0, 1.0)) / total
}

pub fn token_set(text: &str) -> HashSet<String> {
    tokenize(text).into_iter().collect()
}

pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    inter / a.union(b).count() as f32
}

/// One candidate for [`mmr_select`].
pub struct MmrItem {
    pub score: f32,
    /// Document / meeting the chunk belongs to.
    pub source_key: String,
    pub tokens: HashSet<String>,
}

/// Similarity assumed between two chunks of the same source, on top of their text overlap.
pub const SAME_SOURCE_SIMILARITY: f32 = 0.5;

/// Greedy maximal-marginal-relevance selection of up to `k` indices (best first).
/// `lambda = 1.0` keeps pure relevance order; lower values penalise chunks similar to those
/// already picked, where chunks of one source count as at least `SAME_SOURCE_SIMILARITY`
/// similar. `max_per_source > 0` hard-caps chunks per source.
pub fn mmr_select(items: &[MmrItem], k: usize, lambda: f32, max_per_source: usize) -> Vec<usize> {
    let mut selected: Vec<usize> = Vec::new();
    let mut per_source: HashMap<&str, usize> = HashMap::new();
    let mut remaining: Vec<usize> = (0..items.len()).collect();
    while selected.len() < k && !remaining.is_empty() {
        let mut best: Option<(usize, f32)> = None;
        for (pos, &i) in remaining.iter().enumerate() {
            let it = &items[i];
            if max_per_source > 0
                && per_source.get(it.source_key.as_str()).copied().unwrap_or(0) >= max_per_source
            {
                continue;
            }
            let redundancy = selected
                .iter()
                .map(|&j| {
                    let o = &items[j];
                    let text_sim = jaccard(&it.tokens, &o.tokens);
                    if o.source_key == it.source_key {
                        text_sim.max(SAME_SOURCE_SIMILARITY)
                    } else {
                        text_sim
                    }
                })
                .fold(0.0f32, f32::max);
            let mmr = lambda * it.score - (1.0 - lambda) * redundancy;
            if best.is_none_or(|(_, s)| mmr > s) {
                best = Some((pos, mmr));
            }
        }
        let Some((pos, _)) = best else { break };
        let i = remaining.remove(pos);
        *per_source.entry(items[i].source_key.as_str()).or_insert(0) += 1;
        selected.push(i);
    }
    selected
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn lexical_prefers_term_matches() {
        let docs = [
            "the quarterly budget was approved by finance",
            "lunch menu for friday includes pasta",
            "budget budget budget",
        ];
        let s = lexical_scores("quarterly budget approval", &docs);
        assert!(s[0] > s[1], "{s:?}");
        assert!(s[0] > 0.5, "{s:?}");
        assert_eq!(s[1], 0.0);
        assert!(s.iter().all(|x| (0.0..=1.0).contains(x)));
        assert_eq!(lexical_scores("", &docs), vec![0.0; 3]);
        assert!(lexical_scores("x", &[]).is_empty());
    }

    #[test]
    fn fusion_weights() {
        assert!((fuse(0.5, 1.0, 0.5, 0.5) - 0.75).abs() < 1e-6);
        assert!((fuse(0.8, 0.0, 1.0, 0.0) - 0.8).abs() < 1e-6);
        assert!((fuse(-0.3, 0.0, 1.0, 1.0) - 0.0).abs() < 1e-6);
    }

    fn item(score: f32, src: &str, text: &str) -> MmrItem {
        MmrItem {
            score,
            source_key: src.into(),
            tokens: token_set(text),
        }
    }

    #[test]
    fn mmr_spreads_across_sources() {
        let items = vec![
            item(0.90, "A", "alpha plan budget"),
            item(0.88, "A", "alpha plan hiring"),
            item(0.80, "B", "vendor contract renewal"),
        ];
        assert_eq!(mmr_select(&items, 2, 1.0, 0), [0, 1]);
        assert_eq!(mmr_select(&items, 2, 0.7, 0), [0, 2]);
        // hard cap per source
        assert_eq!(mmr_select(&items, 3, 1.0, 1), [0, 2]);
        assert!(mmr_select(&[], 3, 0.7, 0).is_empty());
    }

    #[test]
    fn jaccard_basics() {
        let a = token_set("alpha beta gamma");
        let b = token_set("beta gamma delta");
        let j = jaccard(&a, &b);
        assert!((j - 0.5).abs() < 1e-6, "{j}");
        assert_eq!(jaccard(&a, &a), 1.0);
    }
}
