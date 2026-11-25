//! Redis adapters: `stt:results` writer with per-segment dedup, fixed-window rate limiter.

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use warptalk_contracts::events::SttResult;
use warptalk_contracts::streams::{self, STT_RESULTS};
use warptalk_kernel::{AppError, AppResult};

use chrono::{DateTime, Utc};
use warptalk_kernel::{RoomId, UserId};

use crate::application::ports::{
    AudioTicketClaims, AudioTicketStore, Published, RateLimiter, SegmentSink, USAGE_STREAM,
    UsageEvent, UsageSink,
};

/// Approximate bound of the `stt:results` streams (same as the pipeline).
const STT_STREAM_MAXLEN: usize = 100_000;
/// Remembered client segment ids live this long (a session is well within a day).
const DEDUP_TTL_SECS: u64 = 172_800;

fn bus_err(e: impl std::fmt::Display) -> AppError {
    AppError::unavailable(format!("redis unavailable: {e}"))
}

pub struct RedisSegmentSink {
    conn: ConnectionManager,
}

impl RedisSegmentSink {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl SegmentSink for RedisSegmentSink {
    async fn publish(&self, result: &SttResult, dedup_key: &str) -> AppResult<Published> {
        let mut conn = self.conn.clone();
        let key = format!("copilot:seg:{dedup_key}");
        let claimed: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(DEDUP_TTL_SECS)
            .query_async(&mut conn)
            .await
            .map_err(bus_err)?;
        if claimed.is_none() {
            return Ok(Published::Duplicate);
        }
        let copy = streams::per_meeting(STT_RESULTS, result.meeting_id);
        let mut copy_conn = self.conn.clone();
        let (global, per_meeting) = tokio::join!(
            warptalk_bus::xadd_json(&mut conn, STT_RESULTS, result, STT_STREAM_MAXLEN),
            warptalk_bus::xadd_json(&mut copy_conn, &copy, result, STT_STREAM_MAXLEN),
        );
        if let Err(e) = global {
            // Nothing was published: free the id so the client's retry is accepted.
            let _: Result<(), _> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
            return Err(bus_err(e));
        }
        if let Err(e) = per_meeting {
            // The global stream (the one consumers read) has it; the copy is best effort.
            tracing::warn!(meeting = %result.meeting_id, error = %e, "per-meeting stt copy not written");
        }
        Ok(Published::Written)
    }
}

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
    async fn allow(&self, key: &str, limit: u32, window_secs: u64) -> AppResult<bool> {
        let mut conn = self.conn.clone();
        let key = format!("ratelimit:{key}");
        // Create the counter with its expiry first so a crash can never leave an immortal key.
        let _: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg(0)
            .arg("NX")
            .arg("EX")
            .arg(window_secs)
            .query_async(&mut conn)
            .await
            .map_err(bus_err)?;
        let n: i64 = redis::cmd("INCR")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .map_err(bus_err)?;
        Ok(n <= i64::from(limit))
    }
}

/// Approximate bound of `usage:events`.
const USAGE_STREAM_MAXLEN: usize = 200_000;

/// Publishes usage events to the `usage:events` stream.
pub struct RedisUsageSink {
    conn: ConnectionManager,
}

impl RedisUsageSink {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl UsageSink for RedisUsageSink {
    async fn emit(&self, event: &UsageEvent) -> AppResult<()> {
        let mut conn = self.conn.clone();
        warptalk_bus::xadd_json(&mut conn, USAGE_STREAM, event, USAGE_STREAM_MAXLEN)
            .await
            .map(|_| ())
            .map_err(bus_err)
    }
}

/// One-time audio-socket tickets in Redis (`SET .. EX`, consumed with `GETDEL`).
pub struct RedisTicketStore {
    conn: ConnectionManager,
}

impl RedisTicketStore {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl AudioTicketStore for RedisTicketStore {
    async fn put(
        &self,
        key: &str,
        claims: AudioTicketClaims,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let ttl = (expires_at - Utc::now()).num_seconds().max(1);
        let value = format!("{}:{}:{}", claims.user, claims.session, expires_at.timestamp());
        redis::cmd("SET")
            .arg(format!("copilot:audio-ticket:{key}"))
            .arg(value)
            .arg("EX")
            .arg(ttl)
            .query_async::<()>(&mut conn)
            .await
            .map_err(bus_err)
    }

    async fn take(&self, key: &str, now: DateTime<Utc>) -> AppResult<Option<AudioTicketClaims>> {
        let mut conn = self.conn.clone();
        let v: Option<String> = redis::cmd("GETDEL")
            .arg(format!("copilot:audio-ticket:{key}"))
            .query_async(&mut conn)
            .await
            .map_err(bus_err)?;
        let Some(v) = v else { return Ok(None) };
        let mut it = v.split(':');
        let (Some(u), Some(s), Some(exp)) = (it.next(), it.next(), it.next()) else {
            return Ok(None);
        };
        let (Ok(u), Ok(s), Ok(exp)) = (
            u.parse::<uuid::Uuid>(),
            s.parse::<uuid::Uuid>(),
            exp.parse::<i64>(),
        ) else {
            return Ok(None);
        };
        if exp <= now.timestamp() {
            return Ok(None);
        }
        Ok(Some(AudioTicketClaims {
            user: UserId::from(u),
            session: RoomId::from(s),
        }))
    }
}
