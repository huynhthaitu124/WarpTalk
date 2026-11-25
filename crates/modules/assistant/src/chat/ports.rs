//! Ports and small adapters of the chat feature. `apps/api` wires the real ones.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use warptalk_kernel::{RoomId, UserId, WorkspaceId};

use super::model::Citation;

/// A passage from earlier meetings (rolling memory) relevant to the question.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorContextItem {
    /// Short human label, e.g. "Họp kế hoạch Q3, 12/11".
    pub label: String,
    pub text: String,
}

/// Context carried over from earlier meetings of the same series/workspace. The summary
/// module's `MeetingMemory` is adapted to this in `apps/api`.
#[async_trait]
pub trait PriorContextSource: Send + Sync {
    /// At most roughly `max_chars` characters of text in total. Errors read as "nothing".
    async fn prior_context(
        &self,
        workspace: WorkspaceId,
        room: RoomId,
        user: UserId,
        question: &str,
        max_chars: usize,
    ) -> Vec<PriorContextItem>;
}

/// No earlier-meeting context.
pub struct NoopPriorContext;

#[async_trait]
impl PriorContextSource for NoopPriorContext {
    async fn prior_context(
        &self,
        _: WorkspaceId,
        _: RoomId,
        _: UserId,
        _: &str,
        _: usize,
    ) -> Vec<PriorContextItem> {
        Vec::new()
    }
}

/// Fixed-window counter per key.
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// Count one hit; `true` when it is within `limit` for the current `window`.
    /// Infrastructure trouble must read as `true` (never block users on a side system).
    async fn allow(&self, key: &str, limit: u32, window: Duration) -> bool;
}

/// Process-local limiter (tests, API without Redis).
#[derive(Default)]
pub struct MemoryRateLimiter {
    hits: Mutex<HashMap<String, (u32, Instant)>>,
}

#[async_trait]
impl RateLimiter for MemoryRateLimiter {
    async fn allow(&self, key: &str, limit: u32, window: Duration) -> bool {
        let Ok(mut m) = self.hits.lock() else {
            return true;
        };
        let now = Instant::now();
        let e = m.entry(key.to_owned()).or_insert((0, now));
        if now.duration_since(e.1) >= window {
            *e = (0, now);
        }
        e.0 += 1;
        e.0 <= limit
    }
}

/// `INCR` + `EXPIRE` on `assistant:chat:rl:{key}`.
pub struct RedisRateLimiter {
    conn: ConnectionManager,
}

impl RedisRateLimiter {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn allow(&self, key: &str, limit: u32, window: Duration) -> bool {
        let mut conn = self.conn.clone();
        let k = format!("assistant:chat:rl:{key}");
        let n: Result<i64, _> = redis::cmd("INCR").arg(&k).query_async(&mut conn).await;
        match n {
            Ok(n) => {
                if n == 1 {
                    let r: Result<i64, _> = redis::cmd("EXPIRE")
                        .arg(&k)
                        .arg(window.as_secs().max(1))
                        .query_async(&mut conn)
                        .await;
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "chat rate limit expire failed");
                    }
                }
                n <= i64::from(limit)
            }
            Err(e) => {
                tracing::warn!(error = %e, "chat rate limiter unavailable; allowing");
                true
            }
        }
    }
}

/// A finished recap kept briefly so repeated taps cost nothing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CachedRecap {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub model: String,
}

#[async_trait]
pub trait RecapCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<CachedRecap>;
    async fn put(&self, key: &str, value: &CachedRecap, ttl: Duration);
}

/// Process-local cache (tests, API without Redis).
#[derive(Default)]
pub struct MemoryRecapCache {
    map: Mutex<HashMap<String, (CachedRecap, Instant)>>,
}

#[async_trait]
impl RecapCache for MemoryRecapCache {
    async fn get(&self, key: &str) -> Option<CachedRecap> {
        let m = self.map.lock().ok()?;
        m.get(key)
            .filter(|(_, until)| Instant::now() < *until)
            .map(|(v, _)| v.clone())
    }
    async fn put(&self, key: &str, value: &CachedRecap, ttl: Duration) {
        if let Ok(mut m) = self.map.lock() {
            m.insert(key.to_owned(), (value.clone(), Instant::now() + ttl));
        }
    }
}

/// `GET` / `SET EX` of JSON values under `assistant:chat:recap:*`.
pub struct RedisRecapCache {
    conn: ConnectionManager,
}

impl RedisRecapCache {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl RecapCache for RedisRecapCache {
    async fn get(&self, key: &str) -> Option<CachedRecap> {
        let mut conn = self.conn.clone();
        let raw: Option<String> = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| tracing::warn!(error = %e, "recap cache read failed"))
            .ok()
            .flatten();
        serde_json::from_str(&raw?).ok()
    }
    async fn put(&self, key: &str, value: &CachedRecap, ttl: Duration) {
        let Ok(json) = serde_json::to_string(value) else {
            return;
        };
        let mut conn = self.conn.clone();
        let r: Result<(), _> = redis::cmd("SET")
            .arg(key)
            .arg(json)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async(&mut conn)
            .await;
        if let Err(e) = r {
            tracing::warn!(error = %e, "recap cache write failed");
        }
    }
}

/// Earlier-meeting context from the summary feature's `MeetingMemory`.
///
/// Memory is the HOST's (their previous meetings and open items), so only the host of the
/// room gets it; anyone else asking sees none (no leaking of the host's other meetings).
pub struct SummaryMemoryPriorContext {
    summary: Arc<crate::summary::SummaryService>,
}

impl SummaryMemoryPriorContext {
    pub fn new(summary: Arc<crate::summary::SummaryService>) -> Self {
        Self { summary }
    }
}

#[async_trait]
impl PriorContextSource for SummaryMemoryPriorContext {
    async fn prior_context(
        &self,
        workspace: WorkspaceId,
        room: RoomId,
        user: UserId,
        _question: &str,
        max_chars: usize,
    ) -> Vec<PriorContextItem> {
        let host = match self.summary.d.rooms.context(room).await {
            Ok(Some(c)) => c.host_user_id,
            _ => None,
        };
        if host != Some(user) {
            return Vec::new();
        }
        let ctx = match self.summary.prior_context_for_room(workspace, room).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(%room, error = %e, "chat prior context unavailable");
                return Vec::new();
            }
        };
        let mut items = Vec::new();
        let mut used = 0usize;
        let mut push = |label: String, text: String| {
            if used >= max_chars || text.trim().is_empty() {
                return;
            }
            used += text.chars().count();
            items.push(PriorContextItem { label, text });
        };
        for s in &ctx.last_summaries {
            let mut text = s.summary_short.clone();
            if !s.decisions.is_empty() {
                text.push_str(&format!(" Decisions: {}.", s.decisions.join("; ")));
            }
            if !s.open_questions.is_empty() {
                text.push_str(&format!(
                    " Open questions: {}.",
                    s.open_questions.join("; ")
                ));
            }
            push(
                format!(
                    "{} ({})",
                    s.title.as_deref().unwrap_or("earlier meeting"),
                    s.completed_at.format("%Y-%m-%d")
                ),
                text,
            );
        }
        for it in &ctx.open_action_items {
            let mut text = it.task.clone();
            if let Some(o) = &it.owner {
                text.push_str(&format!(" (owner: {o})"));
            }
            if let Some(d) = &it.due {
                text.push_str(&format!(" (due: {d})"));
            }
            push(
                format!(
                    "open action item from {}",
                    it.room_title.as_deref().unwrap_or("an earlier meeting")
                ),
                text,
            );
        }
        items
    }
}
