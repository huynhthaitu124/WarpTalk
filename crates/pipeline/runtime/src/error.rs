use thiserror::Error;

/// Errors surfaced by the runtime's control plane (not the audio path, which never errors).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PipelineError {
    #[error("room pipeline is shut down")]
    RoomClosed,
    #[error("speaker {0} already exists")]
    SpeakerExists(String),
    #[error("speaker {0} not found")]
    SpeakerNotFound(String),
    #[error("invalid pipeline config: {0}")]
    InvalidConfig(String),
}

/// Errors from an STT implementation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SttError {
    #[error("stt connect failed: {0}")]
    Connect(String),
    #[error("stt protocol error: {0}")]
    Protocol(String),
    #[error("stt session closed")]
    Closed,
    #[error("stt timeout: {0}")]
    Timeout(String),
    #[error("stt error: {0}")]
    Other(String),
}
