//! Entities, value objects, state machines. Pure Rust, no IO.

mod routes;
mod status;

pub use routes::{ExistingRoute, NewRoute, RoutePlan, RouteSpeaker, plan_routes};
pub use status::{DomainError, RoomAction, RoomStatus, transition};

use rand::Rng;

/// Participant statuses (spec: 10 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantStatus {
    Invited,
    Waiting,
    Connected,
    Disconnected,
    Left,
    Kicked,
    Rejected,
}

impl ParticipantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Invited => "INVITED",
            Self::Waiting => "WAITING",
            Self::Connected => "CONNECTED",
            Self::Disconnected => "DISCONNECTED",
            Self::Left => "LEFT",
            Self::Kicked => "KICKED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "INVITED" => Self::Invited,
            "WAITING" => Self::Waiting,
            "CONNECTED" => Self::Connected,
            "DISCONNECTED" => Self::Disconnected,
            "LEFT" => Self::Left,
            "KICKED" => Self::Kicked,
            "REJECTED" => Self::Rejected,
            _ => return None,
        })
    }
}

/// Status a joining caller ends up in (spec: 10 §3.2 Join, BR-46).
/// `existing` is the caller's current participant status, if any. `Err` means the join is refused.
pub fn join_status(
    existing: Option<ParticipantStatus>,
    is_host: bool,
    requires_approval: bool,
) -> Result<ParticipantStatus, DomainError> {
    use ParticipantStatus::*;
    match existing {
        Some(Kicked) => return Err(DomainError::Kicked),
        Some(Rejected) if !is_host => return Err(DomainError::Rejected),
        _ => {}
    }
    if is_host {
        return Ok(Connected);
    }
    Ok(match existing {
        Some(Connected) => Connected,
        Some(Waiting) => Waiting,
        // admitted participants may rejoin without re-approval (BR-46)
        Some(Disconnected) | Some(Left) => Connected,
        _ if requires_approval => Waiting,
        _ => Connected,
    })
}

/// Status after a participant leaves (spec: 10 §3.2 Leave). `None` = no-op.
pub fn leave_status(current: ParticipantStatus) -> Option<ParticipantStatus> {
    use ParticipantStatus::*;
    match current {
        Kicked | Rejected => None,
        Waiting => Some(Invited),
        _ => Some(Left),
    }
}

const CODE_LEN: usize = 12;
const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Room code: exactly 12 characters (spec: 10 §4), lowercase letters and digits.
pub fn generate_room_code(rng: &mut impl Rng) -> String {
    (0..CODE_LEN)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// Unique, non-empty language codes preserving order: source first, then targets.
pub fn room_languages(source: &str, targets: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![source.to_owned()];
    for t in targets {
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

/// A language tag is 1..=15 chars of ASCII letters, digits or `-`.
pub fn valid_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 15
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ParticipantStatus::*;

    #[test]
    fn code_is_12_chars() {
        let c = generate_room_code(&mut rand::thread_rng());
        assert_eq!(c.len(), 12);
        assert!(c.bytes().all(|b| CODE_ALPHABET.contains(&b)));
    }

    #[test]
    fn join_rules() {
        assert_eq!(join_status(None, false, true), Ok(Waiting));
        assert_eq!(join_status(None, false, false), Ok(Connected));
        assert_eq!(join_status(None, true, true), Ok(Connected));
        assert_eq!(
            join_status(Some(Kicked), false, false),
            Err(DomainError::Kicked)
        );
        assert_eq!(
            join_status(Some(Kicked), true, false),
            Err(DomainError::Kicked)
        );
        assert_eq!(
            join_status(Some(Rejected), false, false),
            Err(DomainError::Rejected)
        );
        assert_eq!(join_status(Some(Disconnected), false, true), Ok(Connected));
        assert_eq!(join_status(Some(Left), false, true), Ok(Connected));
        assert_eq!(join_status(Some(Invited), false, true), Ok(Waiting));
        assert_eq!(join_status(Some(Waiting), false, false), Ok(Waiting));
    }

    #[test]
    fn leave_rules() {
        assert_eq!(leave_status(Kicked), None);
        assert_eq!(leave_status(Rejected), None);
        assert_eq!(leave_status(Waiting), Some(Invited));
        assert_eq!(leave_status(Connected), Some(Left));
    }

    #[test]
    fn languages() {
        assert_eq!(
            room_languages("vi", &["en".into(), "vi".into(), "en".into()]),
            vec!["vi", "en"]
        );
        assert!(valid_language("en-US"));
        assert!(!valid_language(""));
        assert!(!valid_language("a b"));
    }

    #[test]
    fn status_roundtrip() {
        for s in [
            Invited,
            Waiting,
            Connected,
            Disconnected,
            Left,
            Kicked,
            Rejected,
        ] {
            assert_eq!(ParticipantStatus::parse(s.as_str()), Some(s));
        }
    }
}
