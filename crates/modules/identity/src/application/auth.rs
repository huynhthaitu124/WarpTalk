//! Register, verify, login, refresh, logout.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, Clock, UserId};
use warptalk_web::JwtKeys;

use super::ports::{
    ClientInfo, IdentityRepository, NewRefreshToken, NewUser, RotateOutcome, SessionRecord,
    UserAuthRecord, VerificationMailer,
};
use super::tokens::{generate_token, hash_token};
use crate::domain::password::{hash_password, verify_password};
use crate::domain::{Policy, validation};

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
    ) -> Self {
        Self {
            repo,
            mailer,
            jwt,
            clock,
            policy,
            dummy_hash: hash_password(&generate_token()),
        }
    }

    pub async fn register(&self, cmd: RegisterCommand) -> Result<UserId> {
        let email = validation::normalize_email(&cmd.email)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        let full_name = validation::full_name(&cmd.full_name)
            .map_err(|m| AppError::validation("VALIDATION_ERROR", m))?;
        if cmd.password.chars().count() < self.policy.min_password_len {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                format!(
                    "password must be at least {} characters",
                    self.policy.min_password_len
                ),
            ));
        }
        if cmd.password.len() > 256 {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                "password too long",
            ));
        }
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

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        client: ClientInfo,
    ) -> Result<AuthSession> {
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
        self.issue_session(&rec, Uuid::now_v7(), now, None, client)
            .await
    }

    pub async fn refresh(&self, refresh_token: &str, client: ClientInfo) -> Result<AuthSession> {
        let now = self.clock.now();
        let new_token = generate_token();
        let outcome = self
            .repo
            .rotate_refresh_token(
                &hash_token(refresh_token.trim()),
                &hash_token(&new_token),
                now + self.policy.refresh_ttl,
                now,
                &client,
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
        self.issue_session(&rec, family_id, now, Some(new_token), client)
            .await
    }

    pub async fn logout(&self, refresh_token: &str) -> Result<()> {
        self.repo
            .revoke_family_by_token(&hash_token(refresh_token.trim()), self.clock.now())
            .await?;
        Ok(())
    }

    /// The user's signed-in sessions (one per live refresh-token family), newest activity first.
    pub async fn sessions(&self, user: UserId) -> Result<Vec<SessionRecord>> {
        self.repo.list_sessions(user, self.clock.now()).await
    }

    /// Signs out one of the user's sessions. 404 when it is not theirs or has already ended,
    /// so a session id from another account is indistinguishable from an unknown one.
    pub async fn revoke_session(&self, user: UserId, family_id: Uuid) -> Result<()> {
        if self
            .repo
            .revoke_user_family(user, family_id, self.clock.now())
            .await?
        {
            Ok(())
        } else {
            Err(AppError::coded(
                404,
                "SESSION_NOT_FOUND",
                "session not found",
            ))
        }
    }

    /// Signs out every session of the user except `current`. Returns the revoked token count.
    pub async fn revoke_other_sessions(&self, user: UserId, current: Uuid) -> Result<u64> {
        self.repo
            .revoke_other_families(user, current, self.clock.now())
            .await
    }

    /// `existing_refresh`: already-persisted token (refresh flow); otherwise a new one is stored.
    async fn issue_session(
        &self,
        rec: &UserAuthRecord,
        family: Uuid,
        now: DateTime<Utc>,
        existing_refresh: Option<String>,
        client: ClientInfo,
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
                        client,
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
}
