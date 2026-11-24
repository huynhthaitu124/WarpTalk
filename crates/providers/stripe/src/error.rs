/// Failure talking to Stripe.
#[derive(Debug, thiserror::Error)]
pub enum StripeError {
    #[error("stripe transport error: {0}")]
    Transport(String),
    /// Stripe answered with a non-2xx status. `message` is Stripe's `error.message` (no secrets).
    #[error("stripe api error {status}: {message}")]
    Api {
        status: u16,
        code: Option<String>,
        message: String,
    },
    #[error("unexpected stripe response: {0}")]
    Decode(String),
}

impl StripeError {
    /// 5xx and transport failures are worth retrying; 4xx are not.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Api { status, .. } => *status >= 500 || *status == 429,
            Self::Decode(_) => false,
        }
    }
}
