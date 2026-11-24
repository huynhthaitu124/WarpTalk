//! Invitation mailer adapters.

use std::sync::Arc;

use async_trait::async_trait;
use redis::AsyncCommands;
use warptalk_email::EmailSender;
use warptalk_email::templates::{self, Brand, InvitationInfo, Lang};
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

/// Delivers invitations through an [`EmailSender`] (Resend, ...). The mail links to
/// `{APP_BASE_URL}/invitations/accept?token=`; the raw token is never logged.
pub struct EmailInvitationMailer {
    sender: Arc<dyn EmailSender>,
    brand: Brand,
    lang: Lang,
}

impl EmailInvitationMailer {
    pub fn new(sender: Arc<dyn EmailSender>, brand: Brand, lang: Lang) -> Self {
        Self {
            sender,
            brand,
            lang,
        }
    }
}

#[async_trait]
impl InvitationMailer for EmailInvitationMailer {
    async fn send_invitation(&self, mail: &InvitationMail<'_>) -> AppResult<()> {
        let expires = mail.expires_at.format("%Y-%m-%d").to_string();
        let rendered = templates::workspace_invitation(
            &self.brand,
            self.lang,
            &InvitationInfo {
                workspace: mail.workspace_name,
                inviter: mail.inviter_name,
                role: mail.role.as_str(),
                token: mail.token,
                expires: &expires,
            },
        );
        let id = self
            .sender
            .send(rendered.into_email(mail.to, "workspace_invitation"))
            .await
            .map_err(|e| AppError::Unavailable(format!("invitation email failed: {e}")))?;
        tracing::info!(to = mail.to, message_id = %id.0, "workspace invitation email sent");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Mutex;
    use warptalk_email::{Email, EmailError, MessageId};
    use warptalk_kernel::WorkspaceRole;

    #[derive(Default)]
    struct Capture(Mutex<Vec<Email>>);

    #[async_trait]
    impl EmailSender for Capture {
        async fn send(&self, email: Email) -> Result<MessageId, EmailError> {
            self.0.lock().unwrap().push(email);
            Ok(MessageId("m".into()))
        }
    }

    struct Failing;

    #[async_trait]
    impl EmailSender for Failing {
        async fn send(&self, _: Email) -> Result<MessageId, EmailError> {
            Err(EmailError::Unavailable("down".into()))
        }
    }

    fn mail<'a>(token: &'a str, ws: &'a str) -> InvitationMail<'a> {
        InvitationMail {
            to: "new@example.test",
            workspace_name: ws,
            inviter_name: "Ann",
            role: WorkspaceRole::Member,
            token,
            expires_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn invitation_links_to_accept_page_and_escapes() {
        let cap = Arc::new(Capture::default());
        let m = EmailInvitationMailer::new(
            cap.clone(),
            Brand::new("WarpTalk", "https://app.example.test"),
            Lang::En,
        );
        m.send_invitation(&mail("tok123", "<b>Acme</b>"))
            .await
            .unwrap();
        let sent = cap.0.lock().unwrap();
        assert_eq!(sent[0].to, "new@example.test");
        assert!(
            sent[0]
                .html
                .contains("https://app.example.test/invitations/accept?token=tok123")
        );
        assert!(!sent[0].html.contains("<b>Acme</b>"));
    }

    #[tokio::test]
    async fn delivery_failure_is_reported_for_retry() {
        let m = EmailInvitationMailer::new(
            Arc::new(Failing),
            Brand::new("WarpTalk", "https://x.test"),
            Lang::Vi,
        );
        assert!(m.send_invitation(&mail("t", "W")).await.is_err());
    }
}
