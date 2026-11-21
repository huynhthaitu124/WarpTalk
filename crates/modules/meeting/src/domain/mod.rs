//! Pure meeting rules: statuses, join gate, webhook event kinds. No IO.

pub const STATUS_ACTIVE: &str = "ACTIVE";
pub const STATUS_FINISHED: &str = "FINISHED";

/// Identity of the Google Meet bridge stand-in seat (spec: 11 §6.4).
pub const BRIDGE_IDENTITY: &str = "external-bridge";

/// Ended statuses refuse joins (spec: 11 §5.1 step 2).
pub fn is_ended_status(status: &str) -> bool {
    matches!(status, "ENDED" | "FINISHED" | "CANCELLED" | "EXPIRED")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinGate {
    Allow,
    /// Translation-room participant is still WAITING for admission.
    Lobby,
    Denied,
}

/// Only the host or a CONNECTED participant gets a token.
pub fn join_gate(is_host: bool, participant_status: Option<&str>) -> JoinGate {
    if is_host {
        return JoinGate::Allow;
    }
    match participant_status {
        Some("CONNECTED") => JoinGate::Allow,
        Some("WAITING") => JoinGate::Lobby,
        _ => JoinGate::Denied,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackChange {
    Published,
    Unpublished,
    Muted,
    Unmuted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookKind {
    ParticipantJoined,
    ParticipantLeft,
    Track(TrackChange),
    RoomFinished,
    /// Egress and other events: acknowledged, not acted on in this slice.
    Ignored,
}

impl WebhookKind {
    pub fn parse(event: &str) -> Self {
        match event {
            "participant_joined" => Self::ParticipantJoined,
            "participant_left" => Self::ParticipantLeft,
            "track_published" => Self::Track(TrackChange::Published),
            "track_unpublished" => Self::Track(TrackChange::Unpublished),
            "track_muted" => Self::Track(TrackChange::Muted),
            "track_unmuted" => Self::Track(TrackChange::Unmuted),
            "room_finished" => Self::RoomFinished,
            _ => Self::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate() {
        assert_eq!(join_gate(true, None), JoinGate::Allow);
        assert_eq!(join_gate(false, Some("CONNECTED")), JoinGate::Allow);
        assert_eq!(join_gate(false, Some("WAITING")), JoinGate::Lobby);
        assert_eq!(join_gate(false, Some("KICKED")), JoinGate::Denied);
        assert_eq!(join_gate(false, None), JoinGate::Denied);
    }

    #[test]
    fn ended() {
        assert!(is_ended_status("FINISHED"));
        assert!(!is_ended_status("IN_PROGRESS"));
    }

    #[test]
    fn kinds() {
        assert_eq!(
            WebhookKind::parse("track_muted"),
            WebhookKind::Track(TrackChange::Muted)
        );
        assert_eq!(WebhookKind::parse("egress_ended"), WebhookKind::Ignored);
    }
}
