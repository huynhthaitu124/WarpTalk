use chrono::{DateTime, Utc};

/// Derived account status (BR-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    PendingVerification,
    Active,
    Locked,
    Disabled,
}

impl AccountStatus {
    /// Error code for a refused (non-active) status.
    pub fn refusal_code(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::PendingVerification => Some("EMAIL_NOT_VERIFIED"),
            Self::Locked => Some("ACCOUNT_LOCKED"),
            Self::Disabled => Some("ACCOUNT_DISABLED"),
        }
    }
}

/// Persisted flags the status is derived from.
#[derive(Debug, Clone)]
pub struct AccountFlags {
    pub email_verified: bool,
    pub is_active: bool,
    pub is_locked: bool,
    pub locked_until: Option<DateTime<Utc>>,
    pub deleted: bool,
}

impl AccountFlags {
    pub fn is_locked_at(&self, now: DateTime<Utc>) -> bool {
        self.is_locked || self.locked_until.is_some_and(|t| t > now)
    }

    pub fn status(&self, now: DateTime<Utc>) -> AccountStatus {
        if self.deleted || !self.is_active {
            AccountStatus::Disabled
        } else if self.is_locked_at(now) {
            AccountStatus::Locked
        } else if !self.email_verified {
            AccountStatus::PendingVerification
        } else {
            AccountStatus::Active
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn status_derivation() {
        let now = Utc::now();
        let mut f = AccountFlags {
            email_verified: false,
            is_active: true,
            is_locked: false,
            locked_until: None,
            deleted: false,
        };
        assert_eq!(f.status(now), AccountStatus::PendingVerification);
        f.email_verified = true;
        assert_eq!(f.status(now), AccountStatus::Active);
        f.locked_until = Some(now + Duration::minutes(1));
        assert_eq!(f.status(now), AccountStatus::Locked);
        assert_eq!(f.status(now + Duration::minutes(2)), AccountStatus::Active);
        f.is_active = false;
        assert_eq!(f.status(now), AccountStatus::Disabled);
    }
}
