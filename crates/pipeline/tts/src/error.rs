use thiserror::Error;
use warptalk_cartesia::CartesiaError;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TtsError {
    /// Drop, 5xx, 429, timeout: repeating the request may succeed.
    #[error("transient synthesis error: {0}")]
    Transient(String),
    /// The vendor refused this request (bad voice id, bad language): retrying fails the same way.
    #[error("synthesis refused: {0}")]
    Permanent(String),
    /// The consumer of the audio went away; not a vendor fault.
    #[error("synthesis cancelled")]
    Cancelled,
    #[error("invalid tts config: {0}")]
    InvalidConfig(String),
}

impl TtsError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }

    pub fn outcome(&self) -> warptalk_telemetry::Outcome {
        use warptalk_telemetry::Outcome;
        match self {
            Self::Transient(m) if m.contains("timeout") => Outcome::Timeout,
            Self::Transient(_) | Self::Permanent(_) => Outcome::VendorError,
            Self::Cancelled | Self::InvalidConfig(_) => Outcome::Error,
        }
    }
}

impl From<CartesiaError> for TtsError {
    fn from(e: CartesiaError) -> Self {
        match e {
            CartesiaError::Cancelled => Self::Cancelled,
            e if e.is_transient() => Self::Transient(e.to_string()),
            e => Self::Permanent(e.to_string()),
        }
    }
}
