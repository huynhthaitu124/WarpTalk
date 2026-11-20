//! Value objects and authorization policy. Pure Rust, no IO.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceRole};

pub const MAX_NAME_CHARS: usize = 150;
pub const MAX_DESCRIPTION_CHARS: usize = 1000;

/// Stable ids stored in `workspace_members.role_id` for the three workspace roles.
pub fn role_id(role: WorkspaceRole) -> Uuid {
    match role {
        WorkspaceRole::Owner => Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001),
        WorkspaceRole::Admin => Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0002),
        WorkspaceRole::Member => Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0003),
    }
}

pub fn role_from_id(id: Uuid) -> Option<WorkspaceRole> {
    [
        WorkspaceRole::Owner,
        WorkspaceRole::Admin,
        WorkspaceRole::Member,
    ]
    .into_iter()
    .find(|r| role_id(*r) == id)
}

pub fn validate_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::validation(
            "INVALID_WORKSPACE_NAME",
            format!("workspace name must be 1-{MAX_NAME_CHARS} characters"),
        ));
    }
    Ok(n.to_owned())
}

pub fn validate_description(d: Option<&str>) -> AppResult<Option<String>> {
    match d.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if s.chars().count() > MAX_DESCRIPTION_CHARS => Err(AppError::validation(
            "INVALID_DESCRIPTION",
            format!("description must be at most {MAX_DESCRIPTION_CHARS} characters"),
        )),
        other => Ok(other.map(str::to_owned)),
    }
}

/// Url-safe slug base (ascii alphanumerics, dash separated, at most 80 chars).
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 80 {
            break;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "workspace".to_owned()
    } else {
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceSettings {
    pub max_participants_limit: i32,
    pub allow_guest_access: bool,
    pub default_ai_model: Option<String>,
    pub audio_retention_days: i32,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            max_participants_limit: 50,
            allow_guest_access: false,
            default_ai_model: None,
            audio_retention_days: 30,
        }
    }
}

impl WorkspaceSettings {
    pub fn validate(&self) -> AppResult<()> {
        if !(1..=1000).contains(&self.max_participants_limit) {
            return Err(AppError::validation(
                "INVALID_SETTINGS",
                "maxParticipantsLimit must be between 1 and 1000",
            ));
        }
        if !(0..=3650).contains(&self.audio_retention_days) {
            return Err(AppError::validation(
                "INVALID_SETTINGS",
                "audioRetentionDays must be between 0 and 3650",
            ));
        }
        if self
            .default_ai_model
            .as_deref()
            .is_some_and(|m| m.trim().is_empty() || m.chars().count() > 100)
        {
            return Err(AppError::validation(
                "INVALID_SETTINGS",
                "defaultAiModel must be 1-100 characters",
            ));
        }
        Ok(())
    }
}

/// BR-12: Owner/Admin update settings.
pub fn authorize_update_settings(actor: WorkspaceRole) -> AppResult<()> {
    match actor {
        WorkspaceRole::Owner | WorkspaceRole::Admin => Ok(()),
        WorkspaceRole::Member => Err(AppError::forbidden(
            "only owner or admin can update settings",
        )),
    }
}

/// BR-13: workspace deletion is Owner-only.
pub fn authorize_delete(actor: WorkspaceRole) -> AppResult<()> {
    if actor == WorkspaceRole::Owner {
        Ok(())
    } else {
        Err(AppError::forbidden(
            "only the owner can delete the workspace",
        ))
    }
}

/// BR-14: role changes are Owner-only; Owner is never assigned here (BR-10/11); not self.
pub fn authorize_change_role(
    actor: (UserId, WorkspaceRole),
    target: UserId,
    new_role: WorkspaceRole,
) -> AppResult<()> {
    if actor.1 != WorkspaceRole::Owner {
        return Err(AppError::forbidden("only the owner can change roles"));
    }
    if actor.0 == target {
        return Err(AppError::validation(
            "CANNOT_CHANGE_SELF",
            "you cannot change your own role",
        ));
    }
    if new_role == WorkspaceRole::Owner {
        return Err(AppError::validation(
            "USE_TRANSFER_OWNERSHIP",
            "use ownership transfer to assign the owner role",
        ));
    }
    Ok(())
}

/// BR-10/15: owner cannot be removed; actor must outrank the target.
pub fn authorize_remove(actor: WorkspaceRole, target: WorkspaceRole) -> AppResult<()> {
    if target == WorkspaceRole::Owner {
        return Err(AppError::conflict(
            "OWNER_MUST_TRANSFER",
            "the owner cannot be removed; transfer ownership first",
        ));
    }
    if !actor.can_manage(&target) {
        return Err(AppError::forbidden(
            "insufficient role to remove this member",
        ));
    }
    Ok(())
}

/// BR-10: owner cannot leave without transferring.
pub fn authorize_leave(actor: WorkspaceRole) -> AppResult<()> {
    if actor == WorkspaceRole::Owner {
        return Err(AppError::conflict(
            "OWNER_MUST_TRANSFER",
            "the owner must transfer ownership before leaving",
        ));
    }
    Ok(())
}

/// BR-11: Owner-only; target is a different member.
pub fn authorize_transfer(actor: (UserId, WorkspaceRole), new_owner: UserId) -> AppResult<()> {
    if actor.1 != WorkspaceRole::Owner {
        return Err(AppError::forbidden("only the owner can transfer ownership"));
    }
    if actor.0 == new_owner {
        return Err(AppError::validation(
            "INVALID_NEW_OWNER",
            "new owner must be a different member",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn role_ids_roundtrip() {
        for r in [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
        ] {
            assert_eq!(role_from_id(role_id(r)), Some(r));
        }
        assert_eq!(role_from_id(Uuid::nil()), None);
    }

    #[test]
    fn slug_and_name() {
        assert_eq!(slugify("  Hello,  World! "), "hello-world");
        assert_eq!(slugify("???"), "workspace");
        assert!(validate_name("  ").is_err());
        assert!(validate_name(&"x".repeat(151)).is_err());
        assert_eq!(validate_name(" a ").unwrap(), "a");
    }

    #[test]
    fn settings_bounds() {
        assert!(WorkspaceSettings::default().validate().is_ok());
        let s = WorkspaceSettings {
            max_participants_limit: 0,
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn policies() {
        let (a, b) = (UserId::new(), UserId::new());
        assert!(
            authorize_change_role((a, WorkspaceRole::Admin), b, WorkspaceRole::Member).is_err()
        );
        assert!(
            authorize_change_role((a, WorkspaceRole::Owner), a, WorkspaceRole::Member).is_err()
        );
        assert!(authorize_change_role((a, WorkspaceRole::Owner), b, WorkspaceRole::Owner).is_err());
        assert!(authorize_change_role((a, WorkspaceRole::Owner), b, WorkspaceRole::Admin).is_ok());
        assert!(authorize_remove(WorkspaceRole::Admin, WorkspaceRole::Admin).is_err());
        assert!(authorize_remove(WorkspaceRole::Admin, WorkspaceRole::Member).is_ok());
        assert!(authorize_leave(WorkspaceRole::Owner).is_err());
    }
}
