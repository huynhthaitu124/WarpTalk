//! Knowledge facts: closed category set (spec 06 section 6.3) and tolerant parsing of the
//! extractor's JSON answer. A fact that cannot be tied to a source chunk is dropped.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::text::tokenize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactCategory {
    Decision,
    Requirement,
    Definition,
    Commitment,
    Risk,
    Reference,
}

impl FactCategory {
    pub const ALL: [FactCategory; 6] = [
        Self::Decision,
        Self::Requirement,
        Self::Definition,
        Self::Commitment,
        Self::Risk,
        Self::Reference,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Requirement => "requirement",
            Self::Definition => "definition",
            Self::Commitment => "commitment",
            Self::Risk => "risk",
            Self::Reference => "reference",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let l = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|c| c.as_str() == l)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedFact {
    pub fact: String,
    pub category: FactCategory,
    pub quote: Option<String>,
    pub confidence: f32,
    pub source_chunk_ids: Vec<Uuid>,
}

/// JSON schema handed to the LLM.
pub fn fact_schema(categories: &[FactCategory]) -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["facts"],
        "properties": {
            "facts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["fact", "category", "quote", "confidence", "chunks"],
                    "properties": {
                        "fact": {"type": "string"},
                        "category": {"type": "string", "enum": categories.iter().map(|c| c.as_str()).collect::<Vec<_>>()},
                        "quote": {"type": "string"},
                        "confidence": {"type": "number"},
                        "chunks": {"type": "array", "items": {"type": "integer"}}
                    }
                }
            }
        }
    })
}

fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

fn norm(s: &str) -> String {
    tokenize(s).join(" ")
}

/// Parse `{"facts":[{fact,category,quote,confidence,chunks:[index]}]}`.
///
/// `chunks` are `(id, text)` of the chunks shown to the model; the model refers to them by
/// their 1-based position.
/// Unknown categories, empty facts and facts below `min_confidence` are dropped, as are facts
/// whose source cannot be resolved (by index, else by locating the quote in a chunk).
/// Duplicates are removed; at most `max_facts` are kept (highest confidence first).
pub fn parse_facts(
    raw: &str,
    chunks: &[(Uuid, String)],
    max_facts: usize,
    min_confidence: f32,
) -> Vec<ExtractedFact> {
    let Ok(v) = serde_json::from_str::<Value>(strip_fences(raw)) else {
        return Vec::new();
    };
    let items = match v.get("facts").and_then(Value::as_array) {
        Some(a) => a.clone(),
        None => v.as_array().cloned().unwrap_or_default(),
    };
    let mut out: Vec<ExtractedFact> = Vec::new();
    for item in items {
        let fact = item
            .get("fact")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let Some(category) = item
            .get("category")
            .and_then(Value::as_str)
            .and_then(FactCategory::parse)
        else {
            continue;
        };
        if fact.is_empty() {
            continue;
        }
        let confidence = item
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0) as f32;
        if confidence < min_confidence {
            continue;
        }
        let quote = item
            .get("quote")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_owned);
        let mut ids: Vec<Uuid> = item
            .get("chunks")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_u64)
                    .filter_map(|n| (n as usize).checked_sub(1))
                    .filter_map(|i| chunks.get(i).map(|(id, _)| *id))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty()
            && let Some(q) = &quote
        {
            let nq = norm(q);
            if !nq.is_empty() {
                ids = chunks
                    .iter()
                    .filter(|(_, t)| norm(t).contains(&nq))
                    .map(|(id, _)| *id)
                    .collect();
            }
        }
        ids.dedup();
        if ids.is_empty() {
            continue;
        }
        let key = norm(fact);
        if out.iter().any(|f| norm(&f.fact) == key) {
            continue;
        }
        out.push(ExtractedFact {
            fact: fact.to_owned(),
            category,
            quote,
            confidence,
            source_chunk_ids: ids,
        });
    }
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(max_facts);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn chunks() -> Vec<(Uuid, String)> {
        vec![
            (
                Uuid::now_v7(),
                "We decided to ship on Friday. Budget is 10k.".into(),
            ),
            (
                Uuid::now_v7(),
                "Risk: the vendor may be late with the API.".into(),
            ),
        ]
    }

    #[test]
    fn parses_filters_and_resolves_sources() {
        let c = chunks();
        let raw = format!(
            "```json\n{}\n```",
            serde_json::json!({"facts": [
                {"fact": "Ship on Friday", "category": "Decision", "quote": "ship on Friday", "confidence": 0.9, "chunks": [1]},
                {"fact": "Vendor may be late", "category": "risk", "quote": "the vendor may be late", "confidence": 0.7, "chunks": []},
                {"fact": "Made-up", "category": "opinion", "quote": "x", "confidence": 0.99, "chunks": [1]},
                {"fact": "Unsourced", "category": "risk", "quote": "never said", "confidence": 0.9, "chunks": [9]},
                {"fact": "ship on friday", "category": "decision", "quote": "", "confidence": 0.8, "chunks": [1]},
                {"fact": "Low conf", "category": "reference", "quote": "", "confidence": 0.1, "chunks": [1]},
            ]})
        );
        let facts = parse_facts(&raw, &c, 10, 0.5);
        assert_eq!(facts.len(), 2, "{facts:#?}");
        assert_eq!(facts[0].category, FactCategory::Decision);
        assert_eq!(facts[0].source_chunk_ids, vec![c[0].0]);
        assert_eq!(facts[1].category, FactCategory::Risk);
        assert_eq!(facts[1].source_chunk_ids, vec![c[1].0]);
        assert_eq!(parse_facts(&raw, &c, 1, 0.5).len(), 1);
        assert!(parse_facts("not json", &c, 5, 0.0).is_empty());
    }
}
