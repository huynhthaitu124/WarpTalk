//! Classified provider errors.

use std::time::Duration;

/// Provider-neutral error classification. Messages come from the provider and never contain API keys.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// Transient throttling (HTTP 429 that is *not* a spend limit).
    #[error("rate limited: {message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    /// Provider overloaded or unavailable (529, 503, 5xx).
    #[error("provider overloaded: {0}")]
    Overloaded(String),
    #[error("request timed out")]
    Timeout,
    /// Bad / revoked key, or missing permission (401, 403).
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Spend limit, exhausted credits or billing problem. Distinct from a rate limit: never retry.
    #[error("quota or billing problem: {0}")]
    QuotaOrBilling(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("content filtered: {0}")]
    ContentFiltered(String),
    #[error("context too long: {0}")]
    ContextTooLong(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("decode error: {0}")]
    Decode(String),
}

impl LlmError {
    /// Whether repeating the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Overloaded(_) | Self::Timeout | Self::Transport(_)
        )
    }

    /// Server-provided delay hint, if any.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}
