//! Invitation mailer adapters.

use async_trait::async_trait;
use redis::AsyncCommands;
use warptalk_kernel::{AppError, AppResult};

use crate::application::invitations::{InvitationMail, InvitationMailer};

/// Logs that an invitation would be sent. Never logs the token.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogInvitationMailer;

#[async_trait]
impl InvitationMailer for LogInvitationMailer {
    async fn send_invitation(&self, mail: &InvitationMail<'_>) -> AppResult<()> {
        tracing::info!(
            email = mail.to,
            workspace = mail.workspace_name,
            token_len = mail.token.len(),
            "workspace invitation requested (LogInvitationMailer; token not logged)"
        );
        Ok(())
    }
}

/// Environment variable that enables [`DevInvitationMailCapture`]. Must be exactly `1`.
pub const DEV_MAIL_CAPTURE_ENV: &str = "DEV_MAIL_CAPTURE";

/// DEV/TEST ONLY. Stores the latest RAW invitation token in Redis under
/// `dev:mail:workspace-invite:{email_lowercase}` (TTL 1 h) for E2E suites.
///
/// WARNING: this exposes live invitation secrets to anyone who can read Redis. Never enable in
/// production. Only constructible through [`DevInvitationMailCapture::from_env`], which returns
/// `None` unless `DEV_MAIL_CAPTURE=1`.
#[derive(Clone)]
pub struct DevInvitationMailCapture {
    client: redis::Client,
}

impl DevInvitationMailCapture {
    pub fn from_env(redis_url: &str) -> AppResult<Option<Self>> {
        Self::from_flag(
            std::env::var(DEV_MAIL_CAPTURE_ENV).ok().as_deref(),
            redis_url,
        )
    }

    /// [`Self::from_env`] with the variable's value passed in (enabled only for `"1"`).
    pub fn from_flag(flag: Option<&str>, redis_url: &str) -> AppResult<Option<Self>> {
        if flag != Some("1") {
            return Ok(None);
        }
        let client = redis::Client::open(redis_url)
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        tracing::warn!(
            "DEV_MAIL_CAPTURE=1: raw invitation tokens are stored in Redis (dev/test only)"
        );
        Ok(Some(Self { client }))
    }

    pub fn key(email: &str) -> String {
        format!("dev:mail:workspace-invite:{}", email.to_lowercase())
    }
}

#[async_trait]
impl InvitationMailer for DevInvitationMailCapture {
    async fn send_invitation(&self, mail: &InvitationMail<'_>) -> AppResult<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        conn.set_ex::<_, _, ()>(Self::key(mail.to), mail.token, 3600)
            .await
            .map_err(|e| AppError::internal(format!("dev mail capture redis: {e}")))?;
        Ok(())
    }
}
