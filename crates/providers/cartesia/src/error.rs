use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CartesiaError {
    #[error("cartesia connect failed: {0}")]
    Connect(String),
    #[error("cartesia protocol error: {0}")]
    Protocol(String),
    #[error("cartesia timeout: {0}")]
    Timeout(&'static str),
    #[error("cartesia connection closed")]
    Closed,
    /// The vendor answered with an error. `status` is the HTTP / event status code when known.
    #[error("cartesia api error (status {status:?}): {message}")]
    Api { status: Option<u16>, message: String },
    /// The consumer of the audio went away; nothing is wrong with the vendor.
    #[error("synthesis cancelled by consumer")]
    Cancelled,
    #[error("cartesia not configured: {0}")]
    NotConfigured(&'static str),
}

impl CartesiaError {
    /// Whether repeating the same request may succeed (drop, 5xx, 429, timeout). A 4xx other
    /// than 429 is the vendor refusing this request (bad voice id, bad language): retrying or
    /// not, it fails the same way.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Connect(_) | Self::Timeout(_) | Self::Closed => true,
            Self::Api { status, .. } => match status {
                Some(s) => *s == 429 || *s >= 500 || *s == 408,
                None => true,
            },
            Self::Protocol(_) | Self::Cancelled | Self::NotConfigured(_) => false,
        }
    }
}
