//! Server-side STT fallback: configuration, per-user connection guard and wiring bundle.
//! The socket loop itself is in `http::audio_ws`; ingest semantics reuse
//! `CopilotService::ingest_audio_segment` (same dedup, metering and credit gate as
//! `POST /segments`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use warptalk_kernel::UserId;

use super::ports::{AudioTicketStore, ServerStt};

/// Limits and timings of the audio socket (`COPILOT_AUDIO_*` env, see [`AudioConfig::from_env`]).
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// A socket is closed (reason `max_session_duration`) after this long; clients reconnect.
    pub max_session_secs: u64,
    /// No frame and no provider event for this long closes the socket.
    pub idle_timeout_secs: u64,
    /// Open sockets per user (per API instance).
    pub max_concurrent_per_user: usize,
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
    /// `_MAX_FRAME_BYTES`, `_MAX_UTTERANCE_MS`, `_TICKET_TTL_SECS`, `_FEED_TIMEOUT_MS`;
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
}

/// Counts open sockets per user; the permit releases on drop.
#[derive(Default)]
pub struct ConnectionGuard {
    open: Arc<Mutex<HashMap<UserId, usize>>>,
}

pub struct AudioPermit {
    user: UserId,
    open: Arc<Mutex<HashMap<UserId, usize>>>,
}

impl ConnectionGuard {
    pub fn try_acquire(&self, user: UserId, max: usize) -> Option<AudioPermit> {
        let mut m = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let n = m.entry(user).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(AudioPermit {
            user,
            open: self.open.clone(),
        })
    }

    pub fn open_for(&self, user: UserId) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&user)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for AudioPermit {
    fn drop(&mut self) {
        let mut m = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = m.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.user);
            }
        }
    }
}
