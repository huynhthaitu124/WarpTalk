use chrono::Duration;

/// Tunable authentication policy (BR-03, BR-90). Defaults follow the spec.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Consecutive failed logins that lock the account.
    pub max_failed_logins: i32,
    pub lockout: Duration,
    pub verification_ttl: Duration,
    /// Password-reset token lifetime (BR-90: 30 min).
    pub reset_ttl: Duration,
    /// Verification / reset email sends: cooldown between sends per account (BR-87).
    pub resend_cooldown_secs: u64,
    /// ... and the cap per window (5 per 15 min).
    pub resend_max: u32,
    pub resend_window_secs: u64,
    pub refresh_ttl: Duration,
    pub min_password_len: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_failed_logins: 5,
            lockout: Duration::minutes(15),
            verification_ttl: Duration::minutes(60),
            reset_ttl: Duration::minutes(30),
            resend_cooldown_secs: 60,
            resend_max: 5,
            resend_window_secs: 15 * 60,
            refresh_ttl: Duration::days(30),
            min_password_len: 8,
        }
    }
}
