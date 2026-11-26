//! Wire shapes of `embedding:index_requests` / `embedding:index_results` (spec 12 section 6.2,
//! spec 14 section 3.5). Requests are accepted either as one JSON `data` field or as the flat
//! string fields upstream producers write (`chunks_json`, `"true"`/`"false"` flags).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::chunker::{PageText, SegmentInput};
use super::payload::SourceType;

fn t() -> bool {
    true
}

fn active() -> String {
    "active".to_owned()
}

/// A chunk prepared by the producer (legacy path). `metadata` keys understood:
/// `document_name`/`title`, `page`, `heading_path`, `start_ms`, `end_ms`, `segment_id`,
/// `segment_ids`, `speaker_name`/`speaker`, `translation_room_id`/`meeting_id`, `chunk_index`,
/// `ingestion_revision`/`revision`, `fact`, `fact_category`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawChunk {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

/// Unchunked content the module chunks itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IndexContent {
    /// Pages as delivered by the text extractor (page markers already split).
    Document { pages: Vec<PageText> },
    Transcript {
        #[serde(default)]
        meeting_id: Option<String>,
        segments: Vec<SegmentInput>,
    },
    /// Short free text (glossary term, summary): one chunk per paragraph group.
    Text { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRequest {
    #[serde(default = "new_job_id")]
    pub job_id: String,
    pub workspace_id: Uuid,
    /// Logical collection id of the producer (`workspace_{id}`); echoed in the result. The
    /// physical collection is chosen from the embedding profile.
    #[serde(default)]
    pub collection_id: String,
    pub source_type: SourceType,
    pub source_id: String,
    /// Document title / meeting title (citations).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub chunks: Vec<RawChunk>,
    #[serde(default)]
    pub content: Option<IndexContent>,
    #[serde(default = "t")]
    pub external_llm_allowed: bool,
    #[serde(default = "t")]
    pub ai_retrieval_allowed: bool,
    #[serde(default = "active")]
    pub retention_state: String,
    #[serde(default = "active")]
    pub deletion_state: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub timestamp_ms: i64,
    /// pdf/docx: the producer sent no text; the indexer reads `object_key` through its
    /// `DocumentSource` and runs the text extractors.
    #[serde(default)]
    pub needs_extraction: bool,
    #[serde(default)]
    pub object_key: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
}

fn new_job_id() -> String {
    Uuid::now_v7().to_string()
}

fn flag(v: Option<&String>, default: bool) -> bool {
    match v.map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if matches!(s.as_str(), "true" | "1" | "yes") => true,
        Some(s) if matches!(s.as_str(), "false" | "0" | "no") => false,
        _ => default,
    }
}

impl IndexRequest {
    /// Decode a stream entry: JSON in `data`, else the flat upstream fields.
    pub fn from_fields(fields: &HashMap<String, String>) -> Result<Self, String> {
        if let Some(raw) = fields.get("data") {
            return serde_json::from_str(raw).map_err(|e| format!("invalid `data` JSON: {e}"));
        }
        let get = |k: &str| fields.get(k).cloned();
        let need = |k: &str| get(k).ok_or_else(|| format!("missing field `{k}`"));
        let workspace_id = Uuid::parse_str(need("workspace_id")?.trim())
            .map_err(|e| format!("invalid workspace_id: {e}"))?;
        let source_type: SourceType = need("source_type")?.parse()?;
        let chunks: Vec<RawChunk> = match get("chunks_json") {
            Some(j) => serde_json::from_str(&j).map_err(|e| format!("invalid chunks_json: {e}"))?,
            None => Vec::new(),
        };
        let content = match get("content_json") {
            Some(j) => {
                Some(serde_json::from_str(&j).map_err(|e| format!("invalid content_json: {e}"))?)
            }
            None => None,
        };
        Ok(Self {
            job_id: get("job_id").unwrap_or_else(new_job_id),
            workspace_id,
            collection_id: get("collection_id").unwrap_or_default(),
            source_type,
            source_id: need("source_id")?,
            title: get("title"),
            chunks,
            content,
            external_llm_allowed: flag(fields.get("external_llm_allowed"), true),
            ai_retrieval_allowed: flag(fields.get("ai_retrieval_allowed"), true),
            retention_state: get("retention_state").unwrap_or_else(active),
            deletion_state: get("deletion_state").unwrap_or_else(active),
            revision: get("revision"),
            timestamp_ms: get("timestamp_ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            needs_extraction: flag(fields.get("needs_extraction"), false),
            object_key: get("object_key").filter(|s| !s.is_empty()),
            mime_type: get("mime_type").filter(|s| !s.is_empty()),
            file_name: get("file_name").filter(|s| !s.is_empty()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Indexed,
    Blocked,
    Deleted,
    Failed,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Blocked => "blocked",
            Self::Deleted => "deleted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexResult {
    pub job_id: String,
    pub workspace_id: Uuid,
    pub collection_id: String,
    pub source_type: SourceType,
    pub source_id: String,
    pub status: ResultStatus,
    pub chunks_indexed: u32,
    pub provider: String,
    pub model: String,
    pub dimensions: u32,
    pub reason: String,
    pub timestamp_ms: i64,
    /// Extras (not in the upstream result): PII hits masked / chunks blocked, facts stored.
    #[serde(default)]
    pub pii_masked: u32,
    #[serde(default)]
    pub pii_blocked: u32,
    #[serde(default)]
    pub facts: u32,
    #[serde(default)]
    pub attempts: u32,
}

impl IndexResult {
    /// Flat string fields (upstream shape) plus the full JSON under `data`.
    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("job_id", self.job_id.clone()),
            ("workspace_id", self.workspace_id.to_string()),
            ("collection_id", self.collection_id.clone()),
            ("source_type", self.source_type.as_str().to_owned()),
            ("source_id", self.source_id.clone()),
            ("status", self.status.as_str().to_owned()),
            ("chunks_indexed", self.chunks_indexed.to_string()),
            ("provider", self.provider.clone()),
            ("model", self.model.clone()),
            ("dimensions", self.dimensions.to_string()),
            ("reason", self.reason.clone()),
            ("timestamp_ms", self.timestamp_ms.to_string()),
            ("data", serde_json::to_string(self).unwrap_or_default()),
        ]
    }
}

/// `knowledge:purge_requests` entry: delete vectors (and rows) of a source, or of a whole
/// workspace when `all` is true.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurgeRequest {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub source_type: Option<SourceType>,
    #[serde(default)]
    pub source_id: Option<String>,
    /// Required to purge a workspace without naming a source.
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

impl PurgeRequest {
    pub fn from_fields(fields: &HashMap<String, String>) -> Result<Self, String> {
        if let Some(raw) = fields.get("data") {
            return serde_json::from_str(raw).map_err(|e| format!("invalid `data` JSON: {e}"));
        }
        let workspace_id = Uuid::parse_str(
            fields
                .get("workspace_id")
                .ok_or("missing field `workspace_id`")?
                .trim(),
        )
        .map_err(|e| format!("invalid workspace_id: {e}"))?;
        let source_type = match fields.get("source_type") {
            Some(s) if !s.is_empty() => Some(s.parse()?),
            _ => None,
        };
        Ok(Self {
            workspace_id,
            source_type,
            source_id: fields.get("source_id").filter(|s| !s.is_empty()).cloned(),
            all: flag(fields.get("all"), false),
            reason: fields.get("reason").cloned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_upstream_flat_fields() {
        let ws = Uuid::now_v7();
        let mut f = HashMap::new();
        f.insert("job_id".into(), "doc1:2".into());
        f.insert("workspace_id".into(), ws.to_string());
        f.insert("collection_id".into(), format!("workspace_{ws}"));
        f.insert("source_type".into(), "document".into());
        f.insert("source_id".into(), "doc1".into());
        f.insert(
            "chunks_json".into(),
            json!([{"id": "c1", "text": "hello", "metadata": {"document_name": "A"}}]).to_string(),
        );
        f.insert("ai_retrieval_allowed".into(), "False".into());
        let r = IndexRequest::from_fields(&f).unwrap();
        assert_eq!(r.job_id, "doc1:2");
        assert!(!r.ai_retrieval_allowed);
        assert!(r.external_llm_allowed);
        assert_eq!(r.retention_state, "active");
        assert_eq!(r.chunks[0].metadata["document_name"], "A");
        f.insert("source_type".into(), "weird".into());
        assert!(IndexRequest::from_fields(&f).is_err());
    }

    #[test]
    fn parses_data_json_with_content() {
        let ws = Uuid::now_v7();
        let data = json!({
            "workspace_id": ws, "source_type": "transcript", "source_id": "t1",
            "content": {"type": "transcript", "meeting_id": "m1", "segments": [
                {"segment_id": "s1", "speaker": "Ann", "start_ms": 0, "end_ms": 1000, "text": "hi"}
            ]}
        });
        let mut f = HashMap::new();
        f.insert("data".into(), data.to_string());
        let r = IndexRequest::from_fields(&f).unwrap();
        assert!(r.ai_retrieval_allowed && r.job_id.len() > 10);
        assert!(matches!(r.content, Some(IndexContent::Transcript { .. })));
    }

    #[test]
    fn result_fields_include_status_and_data() {
        let r = IndexResult {
            job_id: "j".into(),
            workspace_id: Uuid::now_v7(),
            collection_id: "c".into(),
            source_type: SourceType::Document,
            source_id: "s".into(),
            status: ResultStatus::Indexed,
            chunks_indexed: 3,
            provider: "p".into(),
            model: "m".into(),
            dimensions: 8,
            reason: String::new(),
            timestamp_ms: 1,
            pii_masked: 0,
            pii_blocked: 0,
            facts: 0,
            attempts: 1,
        };
        let f = r.to_fields();
        assert!(f.iter().any(|(k, v)| *k == "status" && v == "indexed"));
        let data = &f.iter().find(|(k, _)| *k == "data").unwrap().1;
        assert_eq!(serde_json::from_str::<IndexResult>(data).unwrap(), r);
    }

    #[test]
    fn purge_request_requires_scope_flag_for_all() {
        let ws = Uuid::now_v7();
        let mut f = HashMap::new();
        f.insert("workspace_id".into(), ws.to_string());
        f.insert("source_type".into(), "document".into());
        f.insert("source_id".into(), "d1".into());
        let p = PurgeRequest::from_fields(&f).unwrap();
        assert_eq!(p.source_type, Some(SourceType::Document));
        assert!(!p.all);
    }
}
