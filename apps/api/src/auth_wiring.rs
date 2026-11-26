//! Auth and email wiring: which mail sender is used, Google sign-in, abuse throttles.
//!
//! Environment (see docs/RUNBOOK.md):
//! - `DEV_MAIL_CAPTURE=1` -> raw tokens captured in Redis (dev/test only), mails are not sent.
//! - else `RESEND_API_KEY` (+ `EMAIL_FROM`, `APP_BASE_URL`) -> Resend delivery.
//! - else mails are only logged (WARN at startup: nothing is delivered).
//! - `GOOGLE_CLIENT_IDS` (comma list) enables `POST /auth/google`.

use std::sync::Arc;

use anyhow::{Context, bail};
use redis::aio::ConnectionManager;
use warptalk_email::templates::{Brand, Lang};
use warptalk_email::{EmailSender, ResendSender};
use warptalk_identity::{
    DevMailCapture, EmailMailer, GoogleIdVerifier, GoogleTokenVerifier, IdentityExtras, LogMailer,
    RedisThrottle, VerificationMailer,
};
use warptalk_workspace::{
    DevInvitationMailCapture, EmailInvitationMailer, InvitationMailer, LogInvitationMailer,
};

/// Mail-related settings read from the environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailEnv {
    pub dev_capture: bool,
    pub resend_api_key: Option<String>,
    pub email_from: Option<String>,
    pub app_base_url: Option<String>,
    pub brand_name: String,
    pub lang: Option<String>,
}

/// Which delivery path the environment selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailChoice {
    DevCapture,
    Resend,
    LogOnly,
}

impl MailEnv {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Self {
            dev_capture: get("DEV_MAIL_CAPTURE").as_deref() == Some("1"),
            resend_api_key: non_empty("RESEND_API_KEY"),
            email_from: non_empty("EMAIL_FROM"),
            app_base_url: non_empty("APP_BASE_URL"),
            brand_name: non_empty("EMAIL_BRAND_NAME").unwrap_or_else(|| "WarpTalk".to_owned()),
            lang: non_empty("EMAIL_DEFAULT_LANG"),
        }
    }

    pub fn choice(&self) -> MailChoice {
        if self.dev_capture {
            MailChoice::DevCapture
        } else if self.resend_api_key.is_some() {
            MailChoice::Resend
        } else {
            MailChoice::LogOnly
        }
    }
}

/// Builds the verification/reset mailer and the workspace-invitation mailer from the environment.
pub fn mailers(
    redis_url: &str,
) -> anyhow::Result<(Arc<dyn VerificationMailer>, Arc<dyn InvitationMailer>)> {
    mailers_from(&MailEnv::from_env(), redis_url)
}

pub fn mailers_from(
    env: &MailEnv,
    redis_url: &str,
) -> anyhow::Result<(Arc<dyn VerificationMailer>, Arc<dyn InvitationMailer>)> {
    match env.choice() {
        MailChoice::DevCapture => {
            if redis_url.trim().is_empty() {
                bail!("DEV_MAIL_CAPTURE=1 needs REDIS_URL");
            }
            tracing::warn!(
                "DEV_MAIL_CAPTURE enabled: raw tokens stored in Redis, no email is sent (dev/test only)"
            );
            let verify = DevMailCapture::from_flag(Some("1"), redis_url)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .context("dev verification mail capture not enabled")?;
            let invite = DevInvitationMailCapture::from_flag(Some("1"), redis_url)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .context("dev invitation mail capture not enabled")?;
            Ok((Arc::new(verify), Arc::new(invite)))
        }
        MailChoice::Resend => {
            let key = env.resend_api_key.clone().unwrap_or_default();
            let Some(from) = env.email_from.clone() else {
                bail!(
                    "RESEND_API_KEY is set but EMAIL_FROM is not (e.g. 'WarpTalk <no-reply@your-domain>')"
                );
            };
            let Some(base) = env.app_base_url.clone() else {
                bail!("RESEND_API_KEY is set but APP_BASE_URL is not (links in emails need it)");
            };
            let sender: Arc<dyn EmailSender> =
                Arc::new(ResendSender::new(key, from).map_err(|e| anyhow::anyhow!("{e}"))?);
            let brand = Brand::new(env.brand_name.clone(), base);
            let lang = Lang::from_tag(env.lang.as_deref().or(Some("vi")));
            tracing::info!("email delivery via Resend");
            Ok((
                Arc::new(EmailMailer::new(sender.clone(), brand.clone(), lang)),
                Arc::new(EmailInvitationMailer::new(sender, brand, lang)),
            ))
        }
        MailChoice::LogOnly => {
            tracing::warn!(
                "no RESEND_API_KEY (and DEV_MAIL_CAPTURE not set): emails are NOT delivered; \
                 users cannot verify their email or reset passwords"
            );
            Ok((Arc::new(LogMailer), Arc::new(LogInvitationMailer)))
        }
    }
}

/// Google verifier from `GOOGLE_CLIENT_IDS`; `None` (route answers 503) when unset or invalid.
pub fn google_verifier(csv: Option<&str>) -> Option<Arc<dyn GoogleIdVerifier>> {
    match warptalk_google::GoogleVerifier::from_client_ids_csv(csv.unwrap_or("")) {
        None => {
            tracing::warn!(
                "GOOGLE_CLIENT_IDS not set: POST /auth/google answers 503 GOOGLE_NOT_CONFIGURED"
            );
            None
        }
        Some(Ok(v)) => Some(Arc::new(GoogleTokenVerifier(v))),
        Some(Err(e)) => {
            tracing::error!(error = %e, "google sign-in disabled");
            None
        }
    }
}

/// Optional identity collaborators: Redis-backed throttles (when Redis is available) and the
/// Google verifier.
pub fn identity_extras(redis: Option<&ConnectionManager>) -> IdentityExtras {
    IdentityExtras {
        throttle: redis.map(|c| Arc::new(RedisThrottle::new(c.clone())) as Arc<_>),
        google: google_verifier(std::env::var("GOOGLE_CLIENT_IDS").ok().as_deref()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> MailEnv {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        MailEnv::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn sender_selection_precedence() {
        assert_eq!(env(&[]).choice(), MailChoice::LogOnly);
        assert_eq!(
            env(&[("RESEND_API_KEY", "  ")]).choice(),
            MailChoice::LogOnly
        );
        assert_eq!(
            env(&[("RESEND_API_KEY", "re_x")]).choice(),
            MailChoice::Resend
        );
        assert_eq!(
            env(&[("RESEND_API_KEY", "re_x"), ("DEV_MAIL_CAPTURE", "1")]).choice(),
            MailChoice::DevCapture
        );
        // Only the exact value "1" enables capture.
        assert_eq!(
            env(&[("DEV_MAIL_CAPTURE", "true")]).choice(),
            MailChoice::LogOnly
        );
    }

    #[test]
    fn resend_needs_from_and_base_url() {
        let r = |pairs: &[(&str, &str)]| mailers_from(&env(pairs), "redis://localhost").is_err();
        assert!(r(&[("RESEND_API_KEY", "k")]));
        assert!(r(&[("RESEND_API_KEY", "k"), ("EMAIL_FROM", "a@b.c")]));
        assert!(!r(&[
            ("RESEND_API_KEY", "k"),
            ("EMAIL_FROM", "a@b.c"),
            ("APP_BASE_URL", "https://app.test")
        ]));
        assert!(!r(&[]));
    }

    #[test]
    fn google_disabled_without_client_ids() {
        assert!(google_verifier(None).is_none());
        assert!(google_verifier(Some(" , ")).is_none());
        assert!(google_verifier(Some("a.apps.example, b.apps.example")).is_some());
    }
}
