//! Ports: traits the application layer needs from the outside.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId};

use crate::domain::{AccountFlags, Profile, Settings, UserSummary};

/// Delivers the raw email-verification token to the user. Implementations must not log the token.
#[async_trait::async_trait]
pub trait VerificationMailer: Send + Sync {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError>;

    /// Delivers the raw password-reset token (valid 30 min, single use). Adapters that predate
    /// password reset keep this default, which delivers nothing.
    async fn send_password_reset(&self, _email: &str, _token: &str) -> Result<(), AppError> {
        tracing::warn!("this mailer does not deliver password-reset emails");
        Ok(())
    }
}

/// Fixed-window / cooldown counters for abuse limits (BR-87); Redis in production.
#[async_trait::async_trait]
pub trait Throttle: Send + Sync {
    /// Arms a cooldown of `secs`; `true` when none was active (the action may proceed).
    async fn cooldown(&self, key: &str, secs: u64) -> AppResult<bool>;
    /// Counts one hit in a fixed window of `window_secs`; `true` while the count is <= `max`.
    async fn hit(&self, key: &str, window_secs: u64, max: u32) -> AppResult<bool>;
}

/// Claims of a verified Google ID token (see `warptalk-google`).
#[derive(Debug, Clone)]
pub struct GoogleProfile {
    pub sub: String,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Verifies a Google ID token for WarpTalk's OAuth client(s) (BR-04). Errors: 401
/// `INVALID_GOOGLE_TOKEN`, 503 `GOOGLE_UNAVAILABLE`.
#[async_trait::async_trait]
pub trait GoogleIdVerifier: Send + Sync {
    async fn verify(&self, id_token: &str) -> Result<GoogleProfile, AppError>;
}

#[derive(Debug, Clone)]
pub struct NewGoogleUser {
    pub id: UserId,
    pub email: String,
    pub full_name: String,
    pub avatar_url: Option<String>,
    pub google_id: String,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: UserId,
    pub email: String,
    pub password_hash: String,
    pub full_name: String,
    pub now: DateTime<Utc>,
    pub verify_token_hash: String,
    pub verify_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UserAuthRecord {
    pub id: UserId,
    pub email: String,
    pub password_hash: Option<String>,
    /// Linked Google account (`sub`).
    pub google_id: Option<String>,
    pub flags: AccountFlags,
}

#[derive(Debug, Clone)]
pub struct NewRefreshToken {
    pub user_id: UserId,
    pub family_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotateOutcome {
    Rotated {
        user_id: UserId,
        family_id: Uuid,
    },
    /// Token was already rotated/revoked: the whole family has been revoked.
    Reused,
    Expired,
    Unknown,
}

#[async_trait::async_trait]
pub trait IdentityRepository: Send + Sync {
    /// Inserts user + default settings. Duplicate email -> Conflict `EMAIL_TAKEN`.
    async fn insert_user(&self, user: NewUser) -> AppResult<()>;
    async fn find_auth_by_email(&self, email: &str) -> AppResult<Option<UserAuthRecord>>;
    async fn find_auth_by_id(&self, id: UserId) -> AppResult<Option<UserAuthRecord>>;
    async fn record_login_failure(
        &self,
        id: UserId,
        now: DateTime<Utc>,
        max_failures: i32,
        lock_until: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn record_login_success(&self, id: UserId, now: DateTime<Utc>) -> AppResult<()>;
    /// Single-use: marks the email verified and clears the token. False when unknown/expired.
    async fn consume_verification_token(&self, hash: &str, now: DateTime<Utc>) -> AppResult<bool>;
    /// Replaces the verification token (older ones stop working, BR-90). Only for accounts that
    /// are not verified yet.
    async fn set_verification_token(
        &self,
        id: UserId,
        hash: &str,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    /// Replaces the password-reset token (latest-only, BR-90).
    async fn set_reset_token(
        &self,
        id: UserId,
        hash: &str,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    /// Single-use, atomic: when `hash` is the current unexpired reset token, sets the password,
    /// clears the token and lockout counters and revokes ALL the user's refresh tokens (BR-06).
    async fn reset_password_with_token(
        &self,
        hash: &str,
        password_hash: &str,
        now: DateTime<Utc>,
    ) -> AppResult<Option<UserId>>;
    /// Sets the password, drops any pending reset token and revokes every refresh-token family
    /// except `keep_family` (the caller's session).
    async fn change_password(
        &self,
        id: UserId,
        password_hash: &str,
        keep_family: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<()>;

    async fn find_auth_by_google_id(&self, google_id: &str) -> AppResult<Option<UserAuthRecord>>;
    /// Creates a verified, passwordless account bound to a Google identity.
    async fn insert_google_user(&self, user: NewGoogleUser) -> AppResult<()>;
    /// Binds `google_id` to the account. Taken by another account -> Conflict
    /// `GOOGLE_ACCOUNT_IN_USE`.
    async fn link_google(&self, id: UserId, google_id: &str, now: DateTime<Utc>) -> AppResult<()>;
    async fn unlink_google(&self, id: UserId, now: DateTime<Utc>) -> AppResult<()>;

    async fn is_system_admin(&self, id: UserId) -> AppResult<bool>;
    async fn insert_refresh_token(&self, t: NewRefreshToken) -> AppResult<()>;
    /// Atomically rotates `old_hash` into a new token in the same family.
    async fn rotate_refresh_token(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> AppResult<RotateOutcome>;
    /// Revokes the family of the presented token (no-op when unknown).
    async fn revoke_family_by_token(&self, hash: &str, now: DateTime<Utc>) -> AppResult<()>;
    async fn revoke_family(&self, family_id: Uuid, now: DateTime<Utc>) -> AppResult<()>;

    async fn get_profile(&self, id: UserId) -> AppResult<Option<Profile>>;
    async fn update_profile(
        &self,
        id: UserId,
        profile: &Profile,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn get_settings(&self, id: UserId, now: DateTime<Utc>) -> AppResult<Settings>;
    async fn update_settings(
        &self,
        id: UserId,
        settings: &Settings,
        now: DateTime<Utc>,
    ) -> AppResult<()>;

    async fn find_summary_by_id(&self, id: UserId) -> AppResult<Option<UserSummary>>;
    async fn find_summary_by_email(&self, email: &str) -> AppResult<Option<UserSummary>>;
}
