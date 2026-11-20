//! Redis pub/sub channel names.

use warptalk_kernel::RoomId;

// spec: 10 §5.2
pub const TRANSLATION_ROOM_COMMANDS: &str = "warptalk:translation-room:commands";
pub const MEETINGS_EVENTS: &str = "warptalk:meetings:events";
pub const TELEMETRY: &str = "translationRoom:telemetry";
pub const PARTICIPANT_OFFLINE: &str = "translationRoom:participant-offline";
pub const PARTICIPANT_ONLINE: &str = "translationRoom:participant-online";
// spec: 08 §1
pub const STT_SPECULATIVE: &str = "stt:speculative";
// spec: 11 §6.2 (channel is named by the event type)
pub const MEETING_STARTED: &str = "meeting.started";
pub const MEETING_TRACK_PUBLISHED: &str = "meeting.track_published";
pub const MEETING_PARTICIPANT_JOINED: &str = "meeting.participant_joined";

/// `translationRoom:{id}:events` (carries AUDIO_ROUTES_UPDATED).
pub fn room_events(room: RoomId) -> String {
    format!("translationRoom:{room}:events")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn room_events_name() {
        let id = RoomId::new();
        assert_eq!(room_events(id), format!("translationRoom:{id}:events"));
        assert_eq!(TELEMETRY, "translationRoom:telemetry");
    }
}
