//! One flat error enum for the whole crate (UniFFI maps it to a single foreign error type).

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CopilotError {
    #[error("invalid config: {0}")]
    Config(String),
    #[error("copilot is not started")]
    NotStarted,
    #[error("copilot is already started")]
    AlreadyStarted,
    #[error("copilot is stopped")]
    Stopped,
    #[error("stt token error: {0}")]
    Token(String),
    #[error("stt error: {0}")]
    Stt(String),
    #[error("segment sink error: {0}")]
    Sink(String),
    #[error("io error: {0}")]
    Io(String),
}
