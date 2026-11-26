use std::sync::Arc;

use warptalk_email::templates::{self, Brand, Lang};
use warptalk_email::{EmailError, EmailSender};
use warptalk_kernel::AppError;

use crate::application::VerificationMailer;

/// Delivers verification and password-reset mails through an [`EmailSender`] (Resend, ...).
/// The raw token only travels inside the rendered mail; it is never logged.
pub struct EmailMailer {
    sender: Arc<dyn EmailSender>,
    brand: Brand,
    lang: Lang,
}

impl EmailMailer {
    pub fn new(sender: Arc<dyn EmailSender>, brand: Brand, lang: Lang) -> Self {
        Self {
            sender,
            brand,
            lang,
        }
    }
}

fn map_err(e: EmailError) -> AppError {
    AppError::Unavailable(format!("email delivery failed: {e}"))
}

#[async_trait::async_trait]
impl VerificationMailer for EmailMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        let mail =
            templates::verify_email(&self.brand, self.lang, token).into_email(email, "verify");
        self.sender.send(mail).await.map(|_| ()).map_err(map_err)
    }

    async fn send_password_reset(&self, email: &str, token: &str) -> Result<(), AppError> {
        let mail =
            templates::reset_password(&self.brand, self.lang, token).into_email(email, "reset");
        self.sender.send(mail).await.map(|_| ()).map_err(map_err)
    }
}
