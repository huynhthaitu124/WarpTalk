//! Redis adapters: pause gate (spec 12 §5.4) and embedding index publisher (spec 12 §6.2).

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use uuid::Uuid;
use warptalk_contracts::{keys, streams};
use warptalk_kernel::{AppError, AppResult, RoomId};

use crate::application::ports::{EmbeddingIndexRequest, EmbeddingPublisher, PauseGate};

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
