//! Pure meeting rules: statuses, join gate, webhook event kinds. No IO.

pub const STATUS_ACTIVE: &str = "ACTIVE";
pub const STATUS_FINISHED: &str = "FINISHED";

/// Identity of the external bridge stand-in seat (spec: 11 §6.4): the fixed stand-in user id, the
/// same value as `warptalk_room::domain::STAND_IN_USER_ID` (the pipeline takes the far side's
/// speaker id from the LiveKit identity).
pub const BRIDGE_IDENTITY: &str = "00000000-0000-0000-0000-00000000b21d";

/// Who may mint the stand-in token: the current non-stale capturer, or the host when the room
/// has no capturer yet.
pub fn is_bridge_audio_owner(
    capturer: Option<uuid::Uuid>,
    capturer_stale: bool,
    host: uuid::Uuid,
    caller: uuid::Uuid,
) -> bool {
    match capturer {
        Some(c) => c == caller && !capturer_stale,
        None => host == caller,
    }
}

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

/// Chat length cap (spec: 11 §5.4).
pub const MAX_CHAT_CHARS: usize = 1000;
pub const MAX_CLIENT_MESSAGE_ID_CHARS: usize = 100;
pub const MAX_CHAT_PAGE: i64 = 50;

/// Reading chat: the host, or a participant who is in or was in the meeting (a user who merely
/// left keeps read access, spec 11 §5.4); waiting, kicked and rejected users do not.
pub fn can_read_chat(is_host: bool, participant_status: Option<&str>) -> bool {
    is_host
        || matches!(
            participant_status,
            Some("CONNECTED" | "DISCONNECTED" | "LEFT")
        )
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

// ---------------------------------------------------------------- chat language heuristic

/// Cheap script-based language guess for a chat message (HEURISTIC, not real detection):
/// Vietnamese-specific letters -> `vi`, kana/kanji -> `ja`, everything else -> `en`.
pub fn detect_chat_language(text: &str) -> &'static str {
    const VI: &str = "ăâđêôơưĂÂĐÊÔƠƯàáảãạằắẳẵặầấẩẫậèéẻẽẹềếểễệìíỉĩịòóỏõọồốổỗộờớởỡợùúủũụừứửữựỳýỷỹỵ\
                      ÀÁẢÃẠẰẮẲẴẶẦẤẨẪẬÈÉẺẼẸỀẾỂỄỆÌÍỈĨỊÒÓỎÕỌỒỐỔỖỘỜỚỞỠỢÙÚỦŨỤỪỨỬỮỰỲÝỶỸỴ";
    let mut vi = false;
    for c in text.chars() {
        let u = c as u32;
        if (0x3040..=0x30FF).contains(&u) || (0x4E00..=0x9FFF).contains(&u) {
            return "ja";
        }
        if VI.contains(c) {
            vi = true;
        }
    }
    if vi { "vi" } else { "en" }
}

/// Primary language subtag, lowercase (`vi-VN` -> `vi`).
pub fn primary_language(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
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
    fn chat_read_access() {
        assert!(can_read_chat(true, None));
        assert!(can_read_chat(false, Some("LEFT")));
        assert!(!can_read_chat(false, Some("WAITING")));
        assert!(!can_read_chat(false, Some("KICKED")));
        assert!(!can_read_chat(false, None));
    }

    #[test]
    fn bridge_owner() {
        let (a, b) = (uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2));
        assert!(is_bridge_audio_owner(Some(a), false, b, a));
        assert!(!is_bridge_audio_owner(Some(a), true, b, a));
        assert!(!is_bridge_audio_owner(Some(a), false, b, b));
        assert!(is_bridge_audio_owner(None, false, b, b));
        assert!(!is_bridge_audio_owner(None, false, b, a));
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

    #[test]
    fn chat_language_heuristic() {
        assert_eq!(detect_chat_language("Xin chào các bạn"), "vi");
        assert_eq!(detect_chat_language("こんにちは"), "ja");
        assert_eq!(detect_chat_language("hello there"), "en");
        assert_eq!(primary_language("vi-VN"), "vi");
    }
}
