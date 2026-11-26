//! Ports: traits the application layer needs from the outside.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId};

use crate::domain::{AccountFlags, Profile, Settings, UserSummary};

/// Delivers the raw email-verification token to the user. Implementations must not log the token.
#[async_trait::async_trait]
pub trait VerificationMailer: Send + Sync {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError>;
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
    pub flags: AccountFlags,
}

/// Where a sign-in came from, as the request described it. Shown to the user on their sessions
/// page only; never used for an access decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    /// User-Agent, at most 255 chars.
    pub device_info: Option<String>,
    /// A parsed IPv4/IPv6 address, at most 45 chars.
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewRefreshToken {
    pub user_id: UserId,
    pub family_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub client: ClientInfo,
}

/// One signed-in session = one live refresh-token family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub family_id: Uuid,
    pub device_info: Option<String>,
    pub ip_address: Option<String>,
    /// First token of the family (the sign-in).
    pub signed_in_at: DateTime<Utc>,
    /// Newest token of the family (the last refresh).
    pub last_active_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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
    async fn is_system_admin(&self, id: UserId) -> AppResult<bool>;
    async fn insert_refresh_token(&self, t: NewRefreshToken) -> AppResult<()>;
    /// Atomically rotates `old_hash` into a new token in the same family.
    async fn rotate_refresh_token(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
        client: &ClientInfo,
    ) -> AppResult<RotateOutcome>;
    /// Revokes the family of the presented token (no-op when unknown).
    async fn revoke_family_by_token(&self, hash: &str, now: DateTime<Utc>) -> AppResult<()>;
    async fn revoke_family(&self, family_id: Uuid, now: DateTime<Utc>) -> AppResult<()>;
    /// The user's families that still hold an unrevoked, unexpired token; newest activity first.
    async fn list_sessions(
        &self,
        user: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<SessionRecord>>;
    /// Revokes one family of `user`. False when it is not theirs or already ended.
    async fn revoke_user_family(
        &self,
        user: UserId,
        family_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<bool>;
    /// Revokes every family of `user` except `keep`; returns how many tokens were revoked.
    async fn revoke_other_families(
        &self,
        user: UserId,
        keep: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<u64>;

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
