//! Room lifecycle state machine (spec: 10 §3.1).

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomStatus {
    Scheduled,
    Waiting,
    Open,
    InProgress,
    Paused,
    Ended,
    Cancelled,
    Expired,
    Failed,
}

impl RoomStatus {
    pub const ALL: [RoomStatus; 9] = [
        Self::Scheduled,
        Self::Waiting,
        Self::Open,
        Self::InProgress,
        Self::Paused,
        Self::Ended,
        Self::Cancelled,
        Self::Expired,
        Self::Failed,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "SCHEDULED",
            Self::Waiting => "WAITING",
            Self::Open => "OPEN",
            Self::InProgress => "IN_PROGRESS",
            Self::Paused => "PAUSED",
            Self::Ended => "ENDED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// ENDED, CANCELLED, EXPIRED (spec: 10 §3.1 terminal for guards).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended | Self::Cancelled | Self::Expired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomAction {
    /// SCHEDULED -> WAITING.
    OpenWaiting,
    Start,
    Pause,
    Resume,
    StopTranslation,
    End,
    Cancel,
    /// System action (worker).
    Expire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("This room has already ended or has been cancelled.")]
    RoomClosed,
    #[error("Action {action:?} is not allowed while the room is {from:?}.")]
    InvalidTransition { from: RoomStatus, action: RoomAction },
    #[error("You have been permanently removed from this room and cannot rejoin.")]
    Kicked,
    #[error("The host declined your request to join this meeting.")]
    Rejected,
}

/// Next status for `action` applied to `from`. Idempotent re-applications (start/resume/stop on
/// IN_PROGRESS, end on ENDED) succeed without changing the status.
pub fn transition(from: RoomStatus, action: RoomAction) -> Result<RoomStatus, DomainError> {
    use RoomAction as A;
    use RoomStatus as S;
    let next = match (action, from) {
        (A::OpenWaiting, S::Scheduled) => Some(S::Waiting),
        (A::Start, S::Scheduled | S::Waiting | S::Open | S::InProgress) => Some(S::InProgress),
        (A::Pause, S::InProgress) => Some(S::Paused),
        (A::Resume, S::Paused | S::InProgress) => Some(S::InProgress),
        (A::StopTranslation, S::InProgress) => Some(S::InProgress),
        (A::End, S::InProgress | S::Paused | S::Waiting | S::Open | S::Ended) => Some(S::Ended),
        (A::Cancel, S::Scheduled | S::Waiting | S::Open) => Some(S::Cancelled),
        (A::Expire, S::Scheduled | S::Waiting | S::Open) => Some(S::Expired),
        _ => None,
    };
    match next {
        Some(n) => Ok(n),
        None if from.is_terminal() => Err(DomainError::RoomClosed),
        None => Err(DomainError::InvalidTransition { from, action }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use RoomAction as A;
    use RoomStatus as S;

    const ACTIONS: [RoomAction; 8] = [
        A::OpenWaiting,
        A::Start,
        A::Pause,
        A::Resume,
        A::StopTranslation,
        A::End,
        A::Cancel,
        A::Expire,
    ];

    /// Expected result for every (action, from) pair.
    fn expected(action: RoomAction, from: RoomStatus) -> Option<RoomStatus> {
        match action {
            A::OpenWaiting => (from == S::Scheduled).then_some(S::Waiting),
            A::Start => matches!(from, S::Scheduled | S::Waiting | S::Open | S::InProgress)
                .then_some(S::InProgress),
            A::Pause => (from == S::InProgress).then_some(S::Paused),
            A::Resume => matches!(from, S::Paused | S::InProgress).then_some(S::InProgress),
            A::StopTranslation => (from == S::InProgress).then_some(S::InProgress),
            A::End => matches!(
                from,
                S::InProgress | S::Paused | S::Waiting | S::Open | S::Ended
            )
            .then_some(S::Ended),
            A::Cancel => matches!(from, S::Scheduled | S::Waiting | S::Open).then_some(S::Cancelled),
            A::Expire => matches!(from, S::Scheduled | S::Waiting | S::Open).then_some(S::Expired),
        }
    }

    #[test]
    fn full_matrix() {
        for from in S::ALL {
            for action in ACTIONS {
                let got = transition(from, action);
                match expected(action, from) {
                    Some(to) => assert_eq!(got, Ok(to), "{from:?} {action:?}"),
                    None => assert!(got.is_err(), "{from:?} {action:?} should fail"),
                }
            }
        }
    }

    #[test]
    fn terminal_rooms_report_closed() {
        for from in [S::Ended, S::Cancelled, S::Expired] {
            for action in ACTIONS {
                if expected(action, from).is_none() {
                    assert_eq!(transition(from, action), Err(DomainError::RoomClosed));
                }
            }
        }
        assert_eq!(
            transition(S::Paused, A::Start),
            Err(DomainError::InvalidTransition { from: S::Paused, action: A::Start })
        );
    }

    #[test]
    fn status_strings() {
        for s in S::ALL {
            assert_eq!(RoomStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(RoomStatus::parse("x"), None);
        assert!(S::Ended.is_terminal() && !S::Failed.is_terminal() && !S::Paused.is_terminal());
    }
}
