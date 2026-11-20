use chrono::Duration;

/// Tunable authentication policy (BR-03, BR-90). Defaults follow the spec.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Consecutive failed logins that lock the account.
    pub max_failed_logins: i32,
    pub lockout: Duration,
    pub verification_ttl: Duration,
    pub refresh_ttl: Duration,
    pub min_password_len: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_failed_logins: 5,
            lockout: Duration::minutes(15),
            verification_ttl: Duration::minutes(60),
            refresh_ttl: Duration::days(30),
            min_password_len: 8,
        }
    }
}
