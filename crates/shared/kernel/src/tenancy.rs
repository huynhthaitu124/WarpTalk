//! Tenancy primitives: roles and the acting principal.

use crate::ids::{UserId, WorkspaceId};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
}

impl WorkspaceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "OWNER",
            Self::Admin => "ADMIN",
            Self::Member => "MEMBER",
        }
    }

    pub fn rank(&self) -> u8 {
        match self {
            Self::Owner => 3,
            Self::Admin => 2,
            Self::Member => 1,
        }
    }

    /// True if `self` strictly outranks `other`.
    pub fn can_manage(&self, other: &WorkspaceRole) -> bool {
        self.rank() > other.rank()
    }
}

impl FromStr for WorkspaceRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "OWNER" => Ok(Self::Owner),
            "ADMIN" => Ok(Self::Admin),
            "MEMBER" => Ok(Self::Member),
            other => Err(format!("unknown workspace role: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user_id: UserId,
    pub workspace_id: Option<WorkspaceId>,
    pub is_system_admin: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn role_ordering() {
        use WorkspaceRole::*;
        assert!(Owner.rank() > Admin.rank() && Admin.rank() > Member.rank());
        assert!(Owner.can_manage(&Admin));
        assert!(Admin.can_manage(&Member));
        assert!(!Admin.can_manage(&Admin));
        assert!(!Member.can_manage(&Owner));
    }

    #[test]
    fn role_str_roundtrip() {
        for r in [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
        ] {
            assert_eq!(r.as_str().parse::<WorkspaceRole>().unwrap(), r);
        }
        assert!("ROOT".parse::<WorkspaceRole>().is_err());
    }
}
