//! Redis-backed adapters for [`ConsentPolicy`] and [`RoomSignals`].
//!
//! Key names follow the original system (spec 09 sections 4 and 8): the gateway projects the
//! workspace policy into `translationRoom:{room}:ai_policy` and keeps the per-participant listen
//! languages in `translationRoom:{room}:languages`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use warptalk_contracts::keys::transcript_paused_segments;
use warptalk_kernel::{RoomId, WorkspaceId};

use super::ports::{ConsentPolicy, RoomSignals};

pub fn ai_policy_key(room: RoomId) -> String {
    format!("translationRoom:{room}:ai_policy")
}

pub fn languages_key(room: RoomId) -> String {
    format!("translationRoom:{room}:languages")
}

/// Reads `allow_external_llm` from the room's `ai_policy` JSON. FAIL CLOSED: a missing key,
/// unparsable JSON or a Redis error all mean "not allowed".
pub struct RedisConsentPolicy {
    conn: ConnectionManager,
    cache: Mutex<HashMap<RoomId, (bool, Instant)>>,
}

impl RedisConsentPolicy {
    pub fn new(conn: ConnectionManager) -> Self {
        Self {
            conn,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Drop the cached answer of a room that ended.
    pub fn forget(&self, room: RoomId) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(&room);
        }
    }
}

#[async_trait]
impl ConsentPolicy for RedisConsentPolicy {
    async fn external_llm_allowed(
        &self,
        _workspace: WorkspaceId,
        room: RoomId,
        max_age: Duration,
    ) -> bool {
        if let Ok(c) = self.cache.lock()
            && let Some((allowed, at)) = c.get(&room)
            && at.elapsed() < max_age
        {
            return *allowed;
        }
        let mut conn = self.conn.clone();
        let raw: Result<Option<String>, _> = conn.get(ai_policy_key(room)).await;
        let allowed = match raw {
            Ok(Some(raw)) => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v.get("allow_external_llm").and_then(|b| b.as_bool()))
                .unwrap_or(false),
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(%room, error = %e, "ai_policy unreadable; failing closed");
                false
            }
        };
        if let Ok(mut c) = self.cache.lock() {
            c.insert(room, (allowed, Instant::now()));
        }
        allowed
    }
}

pub struct RedisRoomSignals {
    conn: ConnectionManager,
}

impl RedisRoomSignals {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl RoomSignals for RedisRoomSignals {
    async fn transcript_paused(&self, room: RoomId, segment_id: &str) -> bool {
        let mut conn = self.conn.clone();
        conn.sismember(transcript_paused_segments(room), segment_id)
            .await
            .unwrap_or(false)
    }

    async fn reader_languages(&self, room: RoomId) -> Vec<String> {
        let mut conn = self.conn.clone();
        let map: Result<HashMap<String, String>, _> = conn.hgetall(languages_key(room)).await;
        match map {
            Ok(m) => m.into_values().collect(),
            Err(e) => {
                tracing::warn!(%room, error = %e, "reader languages unavailable");
                Vec::new()
            }
        }
    }
}
