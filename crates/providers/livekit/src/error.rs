use thiserror::Error;

#[derive(Debug, Error)]
pub enum LiveKitError {
    #[error("token error: {0}")]
    Token(String),
    #[error("missing webhook authorization")]
    MissingAuth,
    #[error("invalid webhook signature: {0}")]
    InvalidSignature(String),
    #[error("webhook body hash mismatch")]
    BodyHashMismatch,
    #[error("malformed webhook body: {0}")]
    Malformed(String),
    #[error("livekit http error: {0}")]
    Http(String),
    #[error("livekit api error {status} {code}: {msg}")]
    Api {
        status: u16,
        code: String,
        msg: String,
    },
}

impl LiveKitError {
    /// Twirp `not_found` (e.g. deleting a room that no longer exists).
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Api { code, status, .. } if code == "not_found" || *status == 404)
    }

    /// True for webhook authentication failures (map to HTTP 401).
    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            Self::MissingAuth | Self::InvalidSignature(_) | Self::BodyHashMismatch
        )
    }
}
