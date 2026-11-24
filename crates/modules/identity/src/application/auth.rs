//! Register, verify, login, refresh, logout.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, Clock, UserId};
use warptalk_web::JwtKeys;

use super::ports::{
    GoogleIdVerifier, GoogleProfile, IdentityRepository, NewGoogleUser, NewRefreshToken, NewUser,
    RotateOutcome, Throttle, UserAuthRecord, VerificationMailer,
};
use super::tokens::{generate_token, hash_token};
use crate::domain::password::{hash_password, verify_password};
use crate::domain::{AccountStatus, Policy, validation};

type Result<T> = std::result::Result<T, AppError>;

/// 401 with the generic credentials message (never reveals which part was wrong).
fn invalid_credentials() -> AppError {
    AppError::coded(401, "INVALID_CREDENTIALS", "invalid email or password")
}

/// 403 for a refused account status (BR-01).
fn account_refused(code: &str) -> AppError {
    let message = match code {
        "EMAIL_NOT_VERIFIED" => "email address is not verified",
        "ACCOUNT_LOCKED" => "account is temporarily locked",
        _ => "account is disabled",
    };
    AppError::coded(403, code, message)
}

#[derive(Debug, Clone)]
pub struct RegisterCommand {
    pub email: String,
    pub password: String,
    pub full_name: String,
}

#[derive(Debug, Clone)]
pub struct AuthSession {
    pub user_id: UserId,
    pub email: String,
    pub access_token: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub refresh_token: String,
}

pub struct AuthService {
    repo: Arc<dyn IdentityRepository>,
    mailer: Arc<dyn VerificationMailer>,
    jwt: JwtKeys,
    clock: Arc<dyn Clock>,
    policy: Policy,
    throttle: Arc<dyn Throttle>,
    google: Option<Arc<dyn GoogleIdVerifier>>,
    /// Verified against when the email is unknown, to equalise timing.
    dummy_hash: String,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("blocking task failed: {e}")))
}

impl AuthService {
    pub fn new(
        repo: Arc<dyn IdentityRepository>,
        mailer: Arc<dyn VerificationMailer>,
        jwt: JwtKeys,
        clock: Arc<dyn Clock>,
        policy: Policy,
        throttle: Arc<dyn Throttle>,
        google: Option<Arc<dyn GoogleIdVerifier>>,
    ) -> Self {
        Self {
            repo,
            mailer,
            jwt,
            clock,
            policy,
            throttle,
            google,
            dummy_hash: hash_password(&generate_token()),
        }
    }

    pub async fn register(&self, cmd: RegisterCommand) -> Result<UserId> {
        let email = validation::normalize_email(&cmd.email)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        let full_name = validation::full_name(&cmd.full_name)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        self.check_password(&cmd.password)?;
        let password = cmd.password;
        let password_hash = blocking(move || hash_password(&password)).await?;

        let now = self.clock.now();
        let token = generate_token();
        let id = UserId::new();
        self.repo
            .insert_user(NewUser {
                id,
                email: email.clone(),
                password_hash,
                full_name,
                now,
                verify_token_hash: hash_token(&token),
                verify_expires_at: now + self.policy.verification_ttl,
            })
            .await?;
        if let Err(e) = self.mailer.send_verification(&email, &token).await {
            tracing::error!(user_id = %id, error = %e, "failed to send verification email");
        }
        Ok(id)
    }

    /// Password policy (length bounds) shared by register, reset and change.
    fn check_password(&self, password: &str) -> Result<()> {
        if password.chars().count() < self.policy.min_password_len {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                format!(
                    "password must be at least {} characters",
                    self.policy.min_password_len
                ),
            ));
        }
        if password.len() > 256 {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                "password too long",
            ));
        }
        Ok(())
    }

    pub async fn verify_email(&self, token: &str) -> Result<()> {
        let ok = self
            .repo
            .consume_verification_token(&hash_token(token.trim()), self.clock.now())
            .await?;
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(
                "INVALID_TOKEN",
                "verification token is invalid or expired",
            ))
        }
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<AuthSession> {
        let now = self.clock.now();
        let rec = match validation::normalize_email(email) {
            Ok(e) => self.repo.find_auth_by_email(&e).await?,
            Err(_) => None,
        };

        if let Some(r) = &rec
            && !r.flags.deleted
            && r.flags.is_locked_at(now)
        {
            return Err(account_refused("ACCOUNT_LOCKED"));
        }

        let stored = rec
            .as_ref()
            .and_then(|r| r.password_hash.clone())
            .unwrap_or_else(|| self.dummy_hash.clone());
        let known = rec.as_ref().is_some_and(|r| r.password_hash.is_some());
        let pw = password.to_owned();
        let verified = blocking(move || verify_password(&pw, &stored)).await? && known;

        let Some(rec) = rec else {
            return Err(invalid_credentials());
        };
        if !verified {
            self.repo
                .record_login_failure(
                    rec.id,
                    now,
                    self.policy.max_failed_logins,
                    now + self.policy.lockout,
                )
                .await?;
            return Err(invalid_credentials());
        }
        if let Some(code) = rec.flags.status(now).refusal_code() {
            return Err(account_refused(code));
        }
        self.repo.record_login_success(rec.id, now).await?;
        self.issue_session(&rec, Uuid::now_v7(), now, None).await
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<AuthSession> {
        let now = self.clock.now();
        let new_token = generate_token();
        let outcome = self
            .repo
            .rotate_refresh_token(
                &hash_token(refresh_token.trim()),
                &hash_token(&new_token),
                now + self.policy.refresh_ttl,
                now,
            )
            .await?;
        let (user_id, family_id) = match outcome {
            RotateOutcome::Rotated { user_id, family_id } => (user_id, family_id),
            RotateOutcome::Reused => {
                return Err(AppError::coded(
                    401,
                    "REFRESH_TOKEN_REUSED",
                    "refresh token was already used; session revoked",
                ));
            }
            RotateOutcome::Expired => {
                return Err(AppError::coded(
                    401,
                    "REFRESH_TOKEN_EXPIRED",
                    "refresh token expired",
                ));
            }
            RotateOutcome::Unknown => {
                return Err(AppError::coded(
                    401,
                    "INVALID_REFRESH_TOKEN",
                    "invalid refresh token",
                ));
            }
        };
        let rec = self.repo.find_auth_by_id(user_id).await?;
        let Some(rec) = rec else {
            self.repo.revoke_family(family_id, now).await?;
            return Err(AppError::coded(
                401,
                "INVALID_REFRESH_TOKEN",
                "invalid refresh token",
            ));
        };
        if let Some(code) = rec.flags.status(now).refusal_code() {
            self.repo.revoke_family(family_id, now).await?;
            return Err(account_refused(code));
        }
        self.issue_session(&rec, family_id, now, Some(new_token))
            .await
    }

    pub async fn logout(&self, refresh_token: &str) -> Result<()> {
        self.repo
            .revoke_family_by_token(&hash_token(refresh_token.trim()), self.clock.now())
            .await?;
        Ok(())
    }

    /// `existing_refresh`: already-persisted token (refresh flow); otherwise a new one is stored.
    async fn issue_session(
        &self,
        rec: &UserAuthRecord,
        family: Uuid,
        now: DateTime<Utc>,
        existing_refresh: Option<String>,
    ) -> Result<AuthSession> {
        let refresh_token = match existing_refresh {
            Some(t) => t,
            None => {
                let t = generate_token();
                self.repo
                    .insert_refresh_token(NewRefreshToken {
                        user_id: rec.id,
                        family_id: family,
                        token_hash: hash_token(&t),
                        expires_at: now + self.policy.refresh_ttl,
                        now,
                    })
                    .await?;
                t
            }
        };
        let admin = self.repo.is_system_admin(rec.id).await?;
        let (access_token, access_token_expires_at) =
            self.jwt.issue_access(rec.id, family, admin, now)?;
        Ok(AuthSession {
            user_id: rec.id,
            email: rec.email.clone(),
            access_token,
            access_token_expires_at,
            refresh_token,
        })
    }
    /// BR-87: 60 s cooldown and 5 sends per 15 min per account/purpose. A throttle backend
    /// failure blocks the send (fail closed: this guards against email bombing).
    async fn may_send_mail(&self, purpose: &str, email: &str) -> bool {
        let cooldown = format!("auth:{purpose}:cd:{email}");
        let window = format!("auth:{purpose}:win:{email}");
        let res = async {
            Ok::<bool, AppError>(
                self.throttle
                    .cooldown(&cooldown, self.policy.resend_cooldown_secs)
                    .await?
                    && self
                        .throttle
                        .hit(
                            &window,
                            self.policy.resend_window_secs,
                            self.policy.resend_max,
                        )
                        .await?,
            )
        }
        .await;
        match res {
            Ok(allowed) => allowed,
            Err(e) => {
                tracing::error!(error = %e, purpose, "mail throttle unavailable; send skipped");
                false
            }
        }
    }

    /// Re-issues the verification email. Always `Ok` for well-formed emails so callers cannot
    /// probe which addresses are registered (BR-87, BR-90: the previous token stops working).
    pub async fn resend_verification(&self, email: &str) -> Result<()> {
        let email = validation::normalize_email(email)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        if !self.may_send_mail("verify", &email).await {
            return Ok(());
        }
        if let Some(rec) = self.repo.find_auth_by_email(&email).await?
            && !rec.flags.deleted
            && rec.flags.is_active
            && !rec.flags.email_verified
        {
            self.issue_verification(&rec).await;
        }
        Ok(())
    }

    /// Stores a fresh verification token and mails it; delivery failures are logged only.
    async fn issue_verification(&self, rec: &UserAuthRecord) {
        let now = self.clock.now();
        let token = generate_token();
        if let Err(e) = self
            .repo
            .set_verification_token(
                rec.id,
                &hash_token(&token),
                now + self.policy.verification_ttl,
                now,
            )
            .await
        {
            tracing::error!(user_id = %rec.id, error = %e, "failed to store verification token");
            return;
        }
        if let Err(e) = self.mailer.send_verification(&rec.email, &token).await {
            tracing::error!(user_id = %rec.id, error = %e, "failed to send verification email");
        }
    }

    /// Starts a password reset. Always `Ok` for well-formed emails (no account probing). The new
    /// token replaces any earlier one and lives `Policy::reset_ttl` (BR-90).
    pub async fn forgot_password(&self, email: &str) -> Result<()> {
        let email = validation::normalize_email(email)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        if !self.may_send_mail("reset", &email).await {
            return Ok(());
        }
        let Some(rec) = self.repo.find_auth_by_email(&email).await? else {
            return Ok(());
        };
        if rec.flags.deleted || !rec.flags.is_active {
            return Ok(());
        }
        let now = self.clock.now();
        let token = generate_token();
        self.repo
            .set_reset_token(
                rec.id,
                &hash_token(&token),
                now + self.policy.reset_ttl,
                now,
            )
            .await?;
        if let Err(e) = self.mailer.send_password_reset(&rec.email, &token).await {
            tracing::error!(user_id = %rec.id, error = %e, "failed to send password reset email");
        }
        Ok(())
    }

    /// Completes a reset: single-use token, policy-checked password, ALL sessions revoked (BR-06).
    pub async fn reset_password(&self, token: &str, new_password: &str) -> Result<()> {
        self.check_password(new_password)?;
        let pw = new_password.to_owned();
        let hash = blocking(move || hash_password(&pw)).await?;
        let done = self
            .repo
            .reset_password_with_token(&hash_token(token.trim()), &hash, self.clock.now())
            .await?;
        match done {
            Some(_) => Ok(()),
            None => Err(AppError::validation(
                "INVALID_TOKEN",
                "reset token is invalid or expired",
            )),
        }
    }

    /// `POST /me/password`: proves the current password, then revokes every other session
    /// (`keep_family` is the caller's own refresh family).
    pub async fn change_password(
        &self,
        user: UserId,
        keep_family: Uuid,
        current: &str,
        new_password: &str,
    ) -> Result<()> {
        self.check_password(new_password)?;
        let rec = self
            .repo
            .find_auth_by_id(user)
            .await?
            .filter(|r| !r.flags.deleted)
            .ok_or_else(|| AppError::not_found("user not found"))?;
        let Some(stored) = rec.password_hash.clone() else {
            return Err(AppError::conflict(
                "PASSWORD_NOT_SET",
                "this account has no password; use forgot-password to set one",
            ));
        };
        // Brute-force guard for a stolen access token: 5 wrong attempts per 15 min.
        let key = format!("auth:chpw:fail:{user}");
        let cur = current.to_owned();
        let ok = blocking(move || verify_password(&cur, &stored)).await?;
        if !ok {
            if !self.throttle.hit(&key, 900, 5).await? {
                return Err(AppError::RateLimited(
                    "too many failed attempts; try again later".to_owned(),
                ));
            }
            return Err(AppError::validation(
                "INVALID_CURRENT_PASSWORD",
                "current password is incorrect",
            ));
        }
        let pw = new_password.to_owned();
        let hash = blocking(move || hash_password(&pw)).await?;
        self.repo
            .change_password(user, &hash, keep_family, self.clock.now())
            .await
    }

    fn google(&self) -> Result<&Arc<dyn GoogleIdVerifier>> {
        self.google.as_ref().ok_or_else(|| {
            AppError::coded(
                503,
                "GOOGLE_NOT_CONFIGURED",
                "google sign-in is not configured",
            )
        })
    }

    /// `POST /auth/google` (BR-04/05): sign in with a Google ID token. Creates the account when
    /// new; links to an existing local account only when that account's email is verified.
    pub async fn google_login(&self, id_token: &str) -> Result<AuthSession> {
        let g = self.google()?.verify(id_token).await?;
        let now = self.clock.now();

        if let Some(rec) = self.repo.find_auth_by_google_id(&g.sub).await? {
            return self.google_session(&rec, now).await;
        }
        let email = validation::normalize_email(&g.email)
            .map_err(|_| AppError::coded(401, "INVALID_GOOGLE_TOKEN", "invalid google id token"))?;
        for attempt in 0..2 {
            let Some(rec) = self.repo.find_auth_by_email(&email).await? else {
                match self.create_google_user(&g, &email, now).await {
                    Ok(rec) => return self.google_session(&rec, now).await,
                    // Lost a registration race: re-read and take the link path.
                    Err(AppError::Conflict { code, .. })
                        if code == "EMAIL_TAKEN" && attempt == 0 =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            };
            return match rec.flags.status(now) {
                AccountStatus::Disabled => Err(account_refused("ACCOUNT_DISABLED")),
                AccountStatus::PendingVerification => {
                    // BR-04: an unverified local account must not be taken over by a Google
                    // login. Nudge the owner to verify (rate-limited, BR-87).
                    if self.may_send_mail("verify", &email).await {
                        self.issue_verification(&rec).await;
                    }
                    Err(AppError::conflict(
                        "GOOGLE_LINK_REQUIRES_VERIFIED_EMAIL",
                        "verify the existing account's email before signing in with Google",
                    ))
                }
                AccountStatus::Locked => Err(account_refused("ACCOUNT_LOCKED")),
                AccountStatus::Active => {
                    if rec.google_id.is_some() {
                        return Err(AppError::conflict(
                            "GOOGLE_ACCOUNT_MISMATCH",
                            "this account is linked to a different google account",
                        ));
                    }
                    self.repo.link_google(rec.id, &g.sub, now).await?;
                    self.google_session(&rec, now).await
                }
            };
        }
        Err(AppError::internal("google login did not settle"))
    }

    async fn create_google_user(
        &self,
        g: &GoogleProfile,
        email: &str,
        now: DateTime<Utc>,
    ) -> Result<UserAuthRecord> {
        let name = g
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(150).collect::<String>())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_owned());
        let id = UserId::new();
        self.repo
            .insert_google_user(NewGoogleUser {
                id,
                email: email.to_owned(),
                full_name: name,
                avatar_url: g
                    .picture
                    .clone()
                    .filter(|p| p.len() <= 500 && p.starts_with("https://")),
                google_id: g.sub.clone(),
                now,
            })
            .await?;
        self.repo
            .find_auth_by_id(id)
            .await?
            .ok_or_else(|| AppError::internal("google user vanished after insert"))
    }

    async fn google_session(
        &self,
        rec: &UserAuthRecord,
        now: DateTime<Utc>,
    ) -> Result<AuthSession> {
        if let Some(code) = rec.flags.status(now).refusal_code() {
            return Err(account_refused(code));
        }
        self.repo.record_login_success(rec.id, now).await?;
        self.issue_session(rec, Uuid::now_v7(), now, None).await
    }

    /// `POST /me/google/link` (BR-05): the Google email must equal the account email.
    pub async fn link_google(&self, user: UserId, id_token: &str) -> Result<()> {
        let g = self.google()?.verify(id_token).await?;
        let rec = self
            .repo
            .find_auth_by_id(user)
            .await?
            .filter(|r| !r.flags.deleted)
            .ok_or_else(|| AppError::not_found("user not found"))?;
        let g_email = validation::normalize_email(&g.email).unwrap_or_default();
        if g_email != rec.email.to_lowercase() {
            return Err(AppError::conflict(
                "GOOGLE_EMAIL_MISMATCH",
                "the google account's email differs from this account's email",
            ));
        }
        match rec.google_id.as_deref() {
            Some(existing) if existing == g.sub => Ok(()),
            Some(_) => Err(AppError::conflict(
                "GOOGLE_ALREADY_LINKED",
                "a different google account is already linked",
            )),
            None => self.repo.link_google(user, &g.sub, self.clock.now()).await,
        }
    }

    /// `DELETE /me/google/link` (BR-05): refused when the account has no local password.
    pub async fn unlink_google(&self, user: UserId) -> Result<()> {
        let rec = self
            .repo
            .find_auth_by_id(user)
            .await?
            .filter(|r| !r.flags.deleted)
            .ok_or_else(|| AppError::not_found("user not found"))?;
        if rec.google_id.is_none() {
            return Ok(());
        }
        if rec.password_hash.is_none() {
            return Err(AppError::conflict(
                "GOOGLE_UNLINK_REQUIRES_PASSWORD",
                "set a password before unlinking Google",
            ));
        }
        self.repo.unlink_google(user, self.clock.now()).await
    }
}
