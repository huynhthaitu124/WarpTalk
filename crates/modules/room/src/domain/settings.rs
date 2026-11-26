//! Room options (stored in `translation_rooms.settings` jsonb, snake_case keys).

use serde::{Deserialize, Serialize};

/// Host-controlled room options. Missing keys fall back to the defaults below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomSettings {
    /// Non-host joiners wait in the lobby until admitted.
    pub requires_approval: bool,
    /// Participants (not only the host) may start translation.
    pub participants_can_start_translation: bool,
    /// Keep the transcript after the meeting.
    pub save_transcript: bool,
    /// Share the recording/artifacts with participants automatically.
    pub auto_share_record: bool,
}

impl Default for RoomSettings {
    fn default() -> Self {
        Self {
            requires_approval: true,
            participants_can_start_translation: false,
            save_transcript: true,
            auto_share_record: true,
        }
    }
}

/// Partial update of [`RoomSettings`]; `None` keeps the current value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoomSettingsPatch {
    pub requires_approval: Option<bool>,
    pub participants_can_start_translation: Option<bool>,
    pub save_transcript: Option<bool>,
    pub auto_share_record: Option<bool>,
}

impl RoomSettingsPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn apply(&self, base: RoomSettings) -> RoomSettings {
        RoomSettings {
            requires_approval: self.requires_approval.unwrap_or(base.requires_approval),
            participants_can_start_translation: self
                .participants_can_start_translation
                .unwrap_or(base.participants_can_start_translation),
            save_transcript: self.save_transcript.unwrap_or(base.save_transcript),
            auto_share_record: self.auto_share_record.unwrap_or(base.auto_share_record),
        }
    }
}

impl RoomSettings {
    /// Parse the stored jsonb; unknown keys are ignored, missing ones defaulted.
    pub fn from_json(v: &serde_json::Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }
}

/// Most invitations accepted in one request.
pub const MAX_INVITES: usize = 50;

/// Trim, lowercase, validate and dedupe invite emails. `Err` names the first invalid one.
pub fn normalize_invite_emails(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for e in raw {
        let e = e.trim().to_lowercase();
        if e.is_empty() {
            continue;
        }
        if !valid_email(&e) {
            return Err(e);
        }
        if !out.contains(&e) {
            out.push(e);
        }
    }
    Ok(out)
}

fn valid_email(e: &str) -> bool {
    if e.len() > 255 || e.chars().any(char::is_whitespace) {
        return false;
    }
    match e.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_default() {
        let s = RoomSettings::from_json(&serde_json::json!({ "requires_approval": false }));
        assert!(!s.requires_approval);
        assert!(s.save_transcript);
        assert!(s.auto_share_record);
        assert!(!s.participants_can_start_translation);
        assert_eq!(
            RoomSettings::from_json(&serde_json::json!({})),
            RoomSettings::default()
        );
    }

    #[test]
    fn patch_applies_only_given_fields() {
        let p = RoomSettingsPatch {
            save_transcript: Some(false),
            ..Default::default()
        };
        let s = p.apply(RoomSettings::default());
        assert!(!s.save_transcript);
        assert!(s.requires_approval);
        assert!(RoomSettingsPatch::default().is_empty());
    }

    #[test]
    fn emails_normalized() {
        let got = normalize_invite_emails(&[
            " A@Example.com ".into(),
            "a@example.com".into(),
            "".into(),
            "b@x.io".into(),
        ])
        .unwrap();
        assert_eq!(got, vec!["a@example.com", "b@x.io"]);
        assert_eq!(
            normalize_invite_emails(&["nope".into()]).unwrap_err(),
            "nope"
        );
        assert!(normalize_invite_emails(&["a@b".into()]).is_err());
    }
}
