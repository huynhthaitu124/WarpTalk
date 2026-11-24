use redis::AsyncCommands;
use warptalk_kernel::AppError;

use crate::application::VerificationMailer;

/// Environment variable that enables [`DevMailCapture`]. Must be exactly `1`.
pub const DEV_MAIL_CAPTURE_ENV: &str = "DEV_MAIL_CAPTURE";

const TTL_SECONDS: u64 = 3600;

/// DEV/TEST ONLY. Stores the latest RAW verification token in Redis under
/// `dev:mail:verify:{email_lowercase}` (TTL 1 h) so E2E suites can read it back.
///
/// WARNING: this exposes live account-verification secrets to anyone who can read Redis.
/// Never enable in production. It can only be built via [`DevMailCapture::from_env`], which
/// returns `None` unless `DEV_MAIL_CAPTURE=1`.
#[derive(Clone)]
pub struct DevMailCapture {
    client: redis::Client,
}

impl DevMailCapture {
    /// `Ok(Some(..))` only when `DEV_MAIL_CAPTURE=1`; otherwise `Ok(None)` and nothing is created.
    pub fn from_env(redis_url: &str) -> Result<Option<Self>, AppError> {
        Self::from_flag(
            std::env::var(DEV_MAIL_CAPTURE_ENV).ok().as_deref(),
            redis_url,
        )
    }

    /// [`Self::from_env`] with the variable's value passed in (enabled only for `"1"`).
    pub fn from_flag(flag: Option<&str>, redis_url: &str) -> Result<Option<Self>, AppError> {
        if flag != Some("1") {
            return Ok(None);
        }
        let client = redis::Client::open(redis_url)
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        tracing::warn!(
            "DEV_MAIL_CAPTURE=1: raw verification tokens are stored in Redis (dev/test only)"
        );
        Ok(Some(Self { client }))
    }

    pub fn key(purpose: &str, email: &str) -> String {
        format!("dev:mail:{purpose}:{}", email.to_lowercase())
    }
}

impl DevMailCapture {
    async fn store(&self, purpose: &str, email: &str, token: &str) -> Result<(), AppError> {
        tracing::info!(
            email,
            purpose,
            "dev mail captured in redis (token not logged)"
        );
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        conn.set_ex::<_, _, ()>(Self::key(purpose, email), token, TTL_SECONDS)
            .await
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl VerificationMailer for DevMailCapture {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.store("verify", email, token).await
    }

    /// Stored under `dev:mail:reset:{email_lowercase}`.
    async fn send_password_reset(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.store("reset", email, token).await
    }
}
