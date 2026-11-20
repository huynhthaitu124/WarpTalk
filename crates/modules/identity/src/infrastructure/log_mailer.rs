use warptalk_kernel::AppError;

use crate::application::VerificationMailer;

/// Development adapter: logs that a verification mail would be sent, never the token itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogMailer;

#[async_trait::async_trait]
impl VerificationMailer for LogMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        tracing::info!(
            email,
            token_len = token.len(),
            "verification email requested (LogMailer; token not logged)"
        );
        Ok(())
    }
}
