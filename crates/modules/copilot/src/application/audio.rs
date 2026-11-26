//! Server-side STT fallback: configuration, per-user connection guard and wiring bundle.
//! The socket loop itself is in `http::audio_ws`; ingest semantics reuse
//! `CopilotService::ingest_audio_segment` (same dedup, metering and credit gate as
//! `POST /segments`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use warptalk_kernel::{AppResult, UserId};

use super::ports::{AudioTicketStore, ServerStt};

/// Limits and timings of the audio socket (`COPILOT_AUDIO_*` env, see [`AudioConfig::from_env`]).
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// A socket is closed (reason `max_session_duration`) after this long; clients reconnect.
    pub max_session_secs: u64,
    /// No frame and no provider event for this long closes the socket.
    pub idle_timeout_secs: u64,
    /// Open sockets per user (across API instances when the limiter is Redis-backed).
    pub max_concurrent_per_user: usize,
    /// A connection lease expires this long after its last heartbeat (a crashed instance frees
    /// its slots after at most this long).
    pub limit_lease_secs: u64,
    /// When the limiter backend (Redis) is unreachable: `true` admits the socket (availability),
    /// `false` rejects it with 503.
    pub limit_fail_open: bool,
    /// Largest binary frame (channel byte included).
    pub max_frame_bytes: usize,
    /// Audio of one utterance (between commits) per channel.
    pub max_utterance_audio_ms: u64,
    pub ticket_ttl_secs: u64,
    /// `audio-ticket` calls per user per minute.
    pub ticket_limit_per_min: u32,
    /// A provider `feed` slower than this ends the channel's session (backpressure guard).
    pub feed_timeout_ms: u64,
    pub open_timeout_ms: u64,
    /// After the client leaves, wait this long for in-flight transcripts.
    pub drain_timeout_ms: u64,
    /// Server -> client message queue; partials are dropped when it is full.
    pub out_queue: usize,
    pub est_ms_per_char: u64,
    /// Malformed text frames tolerated before the socket is closed.
    pub max_bad_messages: u32,
    pub vocabulary_max_terms: usize,
    pub prompt_max_chars: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            max_session_secs: 3300, // below the provider's 60 min session cap; clients reconnect
            idle_timeout_secs: 120,
            max_concurrent_per_user: 2,
            limit_lease_secs: 60,
            limit_fail_open: true,
            max_frame_bytes: 64_000,
            max_utterance_audio_ms: 5 * 60_000,
            ticket_ttl_secs: 60,
            ticket_limit_per_min: 20,
            feed_timeout_ms: 5_000,
            open_timeout_ms: 10_000,
            drain_timeout_ms: 8_000,
            out_queue: 256,
            est_ms_per_char: 65,
            max_bad_messages: 20,
            vocabulary_max_terms: 100,
            prompt_max_chars: 800,
        }
    }
}

impl AudioConfig {
    /// `COPILOT_AUDIO_MAX_SESSION_SECS`, `_IDLE_TIMEOUT_SECS`, `_MAX_CONCURRENT_PER_USER`,
    /// `_MAX_FRAME_BYTES`, `_MAX_UTTERANCE_MS`, `_TICKET_TTL_SECS`, `_FEED_TIMEOUT_MS`,
    /// `_LIMIT_LEASE_SECS`, `_LIMIT_FAIL_OPEN` (`true`/`false`);
    /// unset or unparsable values keep the default.
    pub fn from_env() -> Self {
        fn num<T: std::str::FromStr>(k: &str, d: T) -> T {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(d)
        }
        let d = Self::default();
        Self {
            max_session_secs: num("COPILOT_AUDIO_MAX_SESSION_SECS", d.max_session_secs),
            idle_timeout_secs: num("COPILOT_AUDIO_IDLE_TIMEOUT_SECS", d.idle_timeout_secs),
            max_concurrent_per_user: num(
                "COPILOT_AUDIO_MAX_CONCURRENT_PER_USER",
                d.max_concurrent_per_user,
            ),
            limit_lease_secs: num("COPILOT_AUDIO_LIMIT_LEASE_SECS", d.limit_lease_secs).max(3),
            limit_fail_open: num("COPILOT_AUDIO_LIMIT_FAIL_OPEN", d.limit_fail_open),
            max_frame_bytes: num("COPILOT_AUDIO_MAX_FRAME_BYTES", d.max_frame_bytes),
            max_utterance_audio_ms: num("COPILOT_AUDIO_MAX_UTTERANCE_MS", d.max_utterance_audio_ms),
            ticket_ttl_secs: num("COPILOT_AUDIO_TICKET_TTL_SECS", d.ticket_ttl_secs),
            feed_timeout_ms: num("COPILOT_AUDIO_FEED_TIMEOUT_MS", d.feed_timeout_ms),
            ..d
        }
    }
}

/// Everything the audio socket needs; give it to `CopilotModule::with_server_stt`.
#[derive(Clone)]
pub struct ServerAudio {
    pub stt: Arc<dyn ServerStt>,
    pub tickets: Arc<dyn AudioTicketStore>,
    pub config: AudioConfig,
    /// Per-user open-socket limit shared by all API replicas (use [`MemoryConnectionLimiter`]
    /// for a single instance or tests).
    pub limiter: Arc<dyn ConnectionLimiter>,
}

/// Per-user open-socket accounting. A slot is a lease with a unique id that must be
/// heartbeated; an unrenewed lease expires, so a crashed instance cannot leak slots forever.
#[async_trait]
pub trait ConnectionLimiter: Send + Sync {
    /// Take a slot unless the user already holds `max`; `Ok(None)` = over the limit. `lease` is
    /// how long the slot lives without a heartbeat.
    async fn acquire(
        &self,
        user: UserId,
        conn_id: &str,
        max: usize,
        lease: Duration,
    ) -> AppResult<Option<()>>;
    /// Extend the slot's lease.
    async fn heartbeat(&self, user: UserId, conn_id: &str, lease: Duration) -> AppResult<()>;
    /// Give the slot back (idempotent).
    async fn release(&self, user: UserId, conn_id: &str) -> AppResult<()>;
}

/// In-process limiter (single instance, tests). Leases do not expire: a permit always releases
/// on drop within the process.
#[derive(Default)]
pub struct MemoryConnectionLimiter {
    open: Mutex<HashMap<UserId, Vec<String>>>,
}

impl MemoryConnectionLimiter {
    pub fn open_for(&self, user: UserId) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&user)
            .map_or(0, Vec::len)
    }
}

#[async_trait]
impl ConnectionLimiter for MemoryConnectionLimiter {
    async fn acquire(
        &self,
        user: UserId,
        conn_id: &str,
        max: usize,
        _lease: Duration,
    ) -> AppResult<Option<()>> {
        let mut m = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let v = m.entry(user).or_default();
        if v.len() >= max {
            return Ok(None);
        }
        v.push(conn_id.to_owned());
        Ok(Some(()))
    }

    async fn heartbeat(&self, _: UserId, _: &str, _: Duration) -> AppResult<()> {
        Ok(())
    }

    async fn release(&self, user: UserId, conn_id: &str) -> AppResult<()> {
        let mut m = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(v) = m.get_mut(&user) {
            v.retain(|c| c != conn_id);
            if v.is_empty() {
                m.remove(&user);
            }
        }
        Ok(())
    }
}

/// A held slot. A background task heartbeats the lease while the socket lives; dropping the
/// permit stops it and releases the slot.
pub struct AudioPermit {
    release: Option<(Arc<dyn ConnectionLimiter>, UserId, String)>,
    heartbeat: Option<JoinHandle<()>>,
}

impl AudioPermit {
    /// A permit that holds nothing (limiter unreachable and `limit_fail_open`).
    pub fn untracked() -> Self {
        Self {
            release: None,
            heartbeat: None,
        }
    }

    pub(crate) fn tracked(
        limiter: Arc<dyn ConnectionLimiter>,
        user: UserId,
        conn_id: String,
        lease: Duration,
    ) -> Self {
        let hb = {
            let (limiter, id) = (limiter.clone(), conn_id.clone());
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(lease / 3);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    if let Err(e) = limiter.heartbeat(user, &id, lease).await {
                        tracing::warn!(error = %e, "audio connection heartbeat failed");
                    }
                }
            })
        };
        Self {
            release: Some((limiter, user, conn_id)),
            heartbeat: Some(hb),
        }
    }
}

impl Drop for AudioPermit {
    fn drop(&mut self) {
        if let Some(h) = self.heartbeat.take() {
            h.abort();
        }
        if let Some((limiter, user, id)) = self.release.take() {
            // The lease expires on its own if the runtime is gone or Redis is down.
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    if let Err(e) = limiter.release(user, &id).await {
                        tracing::warn!(error = %e, "releasing audio connection slot failed");
                    }
                });
            }
        }
    }
}
