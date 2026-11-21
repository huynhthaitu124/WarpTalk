/// Vector store failure. Messages never include credentials.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VectorError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("qdrant returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("decode error: {0}")]
    Decode(String),
    #[error("collection `{0}` does not exist")]
    CollectionNotFound(String),
    #[error("collection `{collection}` has {existing} dimensions but {requested} were requested")]
    DimensionMismatch {
        collection: String,
        existing: usize,
        requested: usize,
    },
    #[error("vector has {got} dimensions, collection expects {expected}")]
    VectorSize { expected: usize, got: usize },
    #[error("invalid collection name `{0}`")]
    InvalidName(String),
}

impl VectorError {
    /// Whether repeating the same call may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Status { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}
