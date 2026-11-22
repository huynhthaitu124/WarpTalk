//! Redis adapters for documents: the index/purge publisher and the `embedding:index_results`
//! consumer that updates document status.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
use warptalk_contracts::streams::{EMBEDDING_INDEX_REQUESTS, EMBEDDING_INDEX_RESULTS};
use warptalk_kernel::{AppError, AppResult, Clock, WorkspaceId};

use crate::application::documents::{
    DocumentIndexPublisher, DocumentIndexRequest, DocumentRepo, IndexOutcome,
    KNOWLEDGE_PURGE_REQUESTS, apply_index_result,
};

const STREAM_MAXLEN: usize = 10_000;

/// Consumer group of the document status updater on `embedding:index_results`.
pub const RESULTS_GROUP: &str = "workspace-document-embedding-results";

pub fn document_results_consumer_config() -> ConsumerConfig {
    let mut c = ConsumerConfig::new(EMBEDDING_INDEX_RESULTS, RESULTS_GROUP);
    // Idle reclaim 30 s, 5 attempts then dead letter (spec 14 section 3.1).
    c.claim_idle_ms = 30_000;
    c.max_deliveries = 5;
    c
}

/// XADDs `{"data": <json>}` entries to `embedding:index_requests` and
/// `knowledge:purge_requests`.
pub struct RedisDocumentIndexPublisher {
    conn: ConnectionManager,
}

impl RedisDocumentIndexPublisher {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

fn bus_err(e: warptalk_bus::BusError) -> AppError {
    AppError::unavailable(format!("event bus unavailable: {e}"))
}

#[async_trait]
impl DocumentIndexPublisher for RedisDocumentIndexPublisher {
    async fn publish_index(&self, req: &DocumentIndexRequest) -> AppResult<()> {
        let mut conn = self.conn.clone();
        warptalk_bus::xadd_json(&mut conn, EMBEDDING_INDEX_REQUESTS, req, STREAM_MAXLEN)
            .await
            .map(|_| ())
            .map_err(bus_err)
    }

    async fn publish_purge(&self, ws: WorkspaceId, doc: Uuid, reason: &str) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let body = json!({
            "workspace_id": ws.as_uuid(),
            "source_type": "document",
            "source_id": doc.to_string(),
            "reason": reason,
        });
        warptalk_bus::xadd_json(&mut conn, KNOWLEDGE_PURGE_REQUESTS, &body, STREAM_MAXLEN)
            .await
            .map(|_| ())
            .map_err(bus_err)
    }
}

/// The fields of an `embedding:index_results` entry this module reads (knowledge `IndexResult`).
#[derive(Debug, Deserialize)]
struct ResultMsg {
    #[serde(default)]
    job_id: String,
    workspace_id: Uuid,
    #[serde(default)]
    source_type: String,
    #[serde(default)]
    source_id: String,
    status: String,
    #[serde(default)]
    chunks_indexed: u32,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    dimensions: u32,
    #[serde(default)]
    reason: String,
}

impl ResultMsg {
    fn from_fields(f: &HashMap<String, String>) -> Result<Self, String> {
        if let Some(raw) = f.get("data") {
            return serde_json::from_str(raw).map_err(|e| format!("invalid `data` JSON: {e}"));
        }
        let get = |k: &str| f.get(k).cloned().unwrap_or_default();
        Ok(Self {
            job_id: get("job_id"),
            workspace_id: Uuid::parse_str(get("workspace_id").trim())
                .map_err(|e| format!("invalid workspace_id: {e}"))?,
            source_type: get("source_type"),
            source_id: get("source_id"),
            status: f.get("status").cloned().ok_or("missing field `status`")?,
            chunks_indexed: get("chunks_indexed").parse().unwrap_or(0),
            provider: get("provider"),
            model: get("model"),
            dimensions: get("dimensions").parse().unwrap_or(0),
            reason: get("reason"),
        })
    }

    /// `{docId}:{revision}` job ids carry the revision; other producers' ids yield `None`.
    fn revision(&self) -> Option<i32> {
        self.job_id.rsplit_once(':')?.1.parse().ok()
    }

    fn outcome(&self) -> Option<IndexOutcome> {
        Some(match self.status.as_str() {
            "indexed" => IndexOutcome::Indexed {
                chunks: self.chunks_indexed,
                version: format!("{}/{}/{}", self.provider, self.model, self.dimensions),
            },
            "blocked" => IndexOutcome::Blocked {
                reason: self.reason.clone(),
            },
            "failed" => IndexOutcome::Failed {
                reason: if self.reason.is_empty() {
                    "embedding_failed".to_owned()
                } else {
                    format!("embedding_failed: {}", self.reason)
                },
            },
            "deleted" => IndexOutcome::Deleted,
            _ => return None,
        })
    }
}

/// Updates document ingestion status from `embedding:index_results` (indexed / blocked /
/// failed / deleted). Results of other sources and of unknown documents are acknowledged.
pub struct DocumentResultHandler {
    repo: Arc<dyn DocumentRepo>,
    clock: Arc<dyn Clock>,
}

impl DocumentResultHandler {
    pub fn new(repo: Arc<dyn DocumentRepo>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, clock }
    }
}

#[async_trait]
impl StreamHandler for DocumentResultHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let res = ResultMsg::from_fields(&msg.fields)
            .map_err(|e| HandlerError::Fatal(format!("undecodable index result: {e}")))?;
        if res.source_type != "document" {
            return Ok(());
        }
        let Ok(doc_id) = Uuid::parse_str(res.source_id.trim()) else {
            return Ok(());
        };
        let Some(outcome) = res.outcome() else {
            return Err(HandlerError::Fatal(format!(
                "unknown index result status `{}`",
                res.status
            )));
        };
        let applied = apply_index_result(
            &*self.repo,
            &*self.clock,
            WorkspaceId::from_uuid(res.workspace_id),
            doc_id,
            res.revision(),
            &outcome,
        )
        .await
        .map_err(|e| HandlerError::Retry(e.to_string()))?;
        if !applied {
            tracing::debug!(document = %doc_id, job = %res.job_id, "index result ignored (unknown, archived or stale)");
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn decodes_flat_and_data_forms() {
        let ws = Uuid::now_v7();
        let doc = Uuid::now_v7();
        let mut f = HashMap::new();
        f.insert("job_id".into(), format!("{doc}:3"));
        f.insert("workspace_id".into(), ws.to_string());
        f.insert("source_type".into(), "document".into());
        f.insert("source_id".into(), doc.to_string());
        f.insert("status".into(), "indexed".into());
        f.insert("chunks_indexed".into(), "4".into());
        f.insert("provider".into(), "p".into());
        f.insert("model".into(), "m".into());
        f.insert("dimensions".into(), "8".into());
        let m = ResultMsg::from_fields(&f).unwrap();
        assert_eq!(m.revision(), Some(3));
        assert_eq!(
            m.outcome(),
            Some(IndexOutcome::Indexed {
                chunks: 4,
                version: "p/m/8".into()
            })
        );

        let data = json!({
            "job_id": "x", "workspace_id": ws, "source_type": "document",
            "source_id": doc.to_string(), "status": "failed", "reason": "boom"
        });
        let mut g = HashMap::new();
        g.insert("data".into(), data.to_string());
        let m = ResultMsg::from_fields(&g).unwrap();
        assert_eq!(m.revision(), None);
        assert!(
            matches!(m.outcome(), Some(IndexOutcome::Failed { reason }) if reason.contains("boom"))
        );
        g.insert("data".into(), "{".into());
        assert!(ResultMsg::from_fields(&g).is_err());
    }
}
