use thiserror::Error;
use warptalk_llm::LlmError;

#[derive(Debug, Error, Clone)]
pub enum MtError {
    #[error("translation timed out ({0})")]
    Timeout(&'static str),
    #[error("realtime connect failed: {0}")]
    Connect(String),
    #[error("realtime protocol error: {0}")]
    Protocol(String),
    #[error("model returned empty text")]
    Empty,
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
    #[error("could not parse model output: {0}")]
    Parse(String),
    #[error("mt not configured: {0}")]
    NotConfigured(String),
    #[error("invalid mt config: {0}")]
    InvalidConfig(String),
}

impl MtError {
    pub fn outcome(&self) -> warptalk_telemetry::Outcome {
        use warptalk_telemetry::Outcome;
        match self {
            Self::Timeout(_) | Self::Llm(LlmError::Timeout) => Outcome::Timeout,
            Self::Llm(_) | Self::Connect(_) | Self::Protocol(_) => Outcome::VendorError,
            _ => Outcome::Error,
        }
    }
}
