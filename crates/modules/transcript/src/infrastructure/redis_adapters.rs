//! Redis adapters: pause gate (spec 12 §5.4) and embedding index publisher (spec 12 §6.2).

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use uuid::Uuid;
use warptalk_contracts::{keys, streams};
use warptalk_kernel::{AppError, AppResult, RoomId};

use crate::application::glossary_ports::{GlossaryFeed, GlossaryScope};
use crate::application::ports::{EmbeddingIndexRequest, EmbeddingPublisher, PauseGate};
use crate::domain::glossary::SnapshotTerm;

/// Remembered paused segment ids expire after 30 minutes (spec 12 §5.4).
pub const PAUSED_SEGMENT_TTL_SECS: i64 = 30 * 60;
/// `embedding:index_requests` approximate max length (spec 12 §6.2).
pub const EMBEDDING_STREAM_MAXLEN: usize = 10_000;

/// Flag key that is present while a room's transcript is paused. It is written by the
/// pause/resume endpoints (a later slice); the shared contracts crate has no key for it yet.
pub fn paused_flag_key(room: RoomId) -> String {
    format!("translationRoom:{room}:transcript_paused")
}

fn redis_err(e: impl std::fmt::Display) -> AppError {
    AppError::unavailable(format!("redis: {e}"))
}

pub struct RedisPauseGate {
    conn: ConnectionManager,
}

impl RedisPauseGate {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl PauseGate for RedisPauseGate {
    async fn is_paused(&self, room: RoomId) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        redis::cmd("EXISTS")
            .arg(paused_flag_key(room))
            .query_async::<i64>(&mut conn)
            .await
            .map(|n| n > 0)
            .map_err(redis_err)
    }

    async fn mark_skipped(&self, room: RoomId, segment: Uuid) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let key = keys::transcript_paused_segments(room);
        redis::pipe()
            .cmd("SADD")
            .arg(&key)
            .arg(segment.to_string())
            .ignore()
            .cmd("EXPIRE")
            .arg(&key)
            .arg(PAUSED_SEGMENT_TTL_SECS)
            .ignore()
            .query_async::<()>(&mut conn)
            .await
            .map_err(redis_err)
    }

    async fn was_skipped(&self, room: RoomId, segment: Uuid) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        redis::cmd("SISMEMBER")
            .arg(keys::transcript_paused_segments(room))
            .arg(segment.to_string())
            .query_async::<bool>(&mut conn)
            .await
            .map_err(redis_err)
    }
}

pub struct RedisEmbeddingPublisher {
    conn: ConnectionManager,
}

impl RedisEmbeddingPublisher {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl EmbeddingPublisher for RedisEmbeddingPublisher {
    async fn publish(&self, r: &EmbeddingIndexRequest) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let fields = [
            ("job_id", r.job_id.to_string()),
            ("workspace_id", r.workspace_id.to_string()),
            ("collection_id", r.collection_id.clone()),
            ("source_type", r.source_type.clone()),
            ("source_id", r.source_id.to_string()),
            ("chunks_json", r.chunks_json.clone()),
            ("external_llm_allowed", r.external_llm_allowed.to_string()),
            ("ai_retrieval_allowed", r.ai_retrieval_allowed.to_string()),
            ("retention_state", r.retention_state.clone()),
            ("deletion_state", r.deletion_state.clone()),
            ("timestamp_ms", r.timestamp_ms.to_string()),
        ];
        warptalk_bus::xadd(
            &mut conn,
            streams::EMBEDDING_INDEX_REQUESTS,
            &fields,
            EMBEDDING_STREAM_MAXLEN,
        )
        .await
        .map(|_| ())
        .map_err(redis_err)
    }
}

/// Pub/sub channel announcing a glossary snapshot change; payload `{scope, id, version}`
/// (`scope` is `workspace` or `global`; `id` is the workspace id, null for global).
pub const GLOSSARY_CHANGED_CHANNEL: &str = "warptalk:glossary:changed";
/// Snapshot of the published global terms.
pub const GLOSSARY_GLOBAL_KEY: &str = "glossary:global";
/// Counter that hands out snapshot versions (strictly increasing across all scopes).
const GLOSSARY_VERSION_COUNTER: &str = "glossary:version";

/// Snapshot key of one workspace's glossary terms.
pub fn glossary_workspace_key(ws: warptalk_kernel::WorkspaceId) -> String {
    format!("glossary:workspace:{ws}")
}

/// Writes `glossary:workspace:{id}` / `glossary:global` as `{version, terms:[...]}` and announces
/// the change on [`GLOSSARY_CHANGED_CHANNEL`]. The pipeline reads these keys (it has no database).
pub struct RedisGlossaryFeed {
    conn: ConnectionManager,
}

impl RedisGlossaryFeed {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl GlossaryFeed for RedisGlossaryFeed {
    async fn publish(&self, scope: &GlossaryScope, terms: &[SnapshotTerm]) -> AppResult<i64> {
        let mut conn = self.conn.clone();
        let version: i64 = redis::cmd("INCR")
            .arg(GLOSSARY_VERSION_COUNTER)
            .query_async(&mut conn)
            .await
            .map_err(redis_err)?;
        let (key, scope_name, id) = match scope {
            GlossaryScope::Workspace(ws) => (
                glossary_workspace_key(*ws),
                "workspace",
                Some(ws.to_string()),
            ),
            GlossaryScope::Global => (GLOSSARY_GLOBAL_KEY.to_owned(), "global", None),
        };
        let body =
            serde_json::to_string(&serde_json::json!({ "version": version, "terms": terms }))
                .map_err(|e| AppError::internal(e.to_string()))?;
        redis::cmd("SET")
            .arg(&key)
            .arg(body)
            .query_async::<()>(&mut conn)
            .await
            .map_err(redis_err)?;
        let announce = serde_json::json!({ "scope": scope_name, "id": id, "version": version });
        warptalk_bus::publish_json(&mut conn, GLOSSARY_CHANGED_CHANNEL, &announce)
            .await
            .map_err(redis_err)?;
        Ok(version)
    }
}
