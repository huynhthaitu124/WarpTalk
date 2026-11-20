//! Redis stream names. Per-meeting copies are `<stream>:{meetingId}` (spec 08 §1).

use warptalk_kernel::RoomId;

// spec: 08 §1
pub const AUDIO_CHUNKS: &str = "audio:chunks";
pub const AUDIO_FRAMES: &str = "audio:frames";
pub const STT_RESULTS: &str = "stt:results";
pub const TRANSLATE_RESULTS: &str = "translate:results";
pub const TTS_RESULTS: &str = "tts:results";

// spec: 12 §6.1
pub const TRANSLATE_BACKFILL_RESULTS: &str = "translate:backfill_results";
pub const TRANSCRIPT_CLEAN: &str = "transcript:clean";
// spec: 12 §6.2
pub const TRANSLATE_BACKFILL_REQUESTS: &str = "translate:backfill_requests";
pub const EMBEDDING_INDEX_REQUESTS: &str = "embedding:index_requests";
pub const EMBEDDING_INDEX_RESULTS: &str = "embedding:index_results";

// spec: 10 §5.3
pub const TRANSLATION_ROOM_SYSTEM_EVENTS: &str = "translationRoom:system_events";
pub const TRANSLATION_ROOM_SYSTEM_EVENTS_DLQ: &str = "translationRoom:system_events:dlq";
pub const MEETING_DOMAIN_EVENTS: &str = "meeting:domain-events";
pub const MEETING_DOMAIN_EVENTS_TRANSLATION_ROOM_DLQ: &str =
    "meeting:domain-events:translation-room:dlq";
pub const WORKSPACE_EVENTS: &str = "workspace-events";
pub const ASSISTANT_SUMMARY_RESULTS: &str = "assistant:summary_results";
pub const ASSISTANT_SUMMARY_REQUESTS: &str = "assistant:summary_requests";
pub const KNOWLEDGE_FACT_REQUESTS: &str = "knowledge:fact_requests";

// spec: 11 §6.3
pub const ASSISTANT_CHAT_REQUESTS: &str = "assistant:chat_requests";
pub const ASSISTANT_CHAT_RESULTS: &str = "assistant:chat_results";
pub const ASSISTANT_CHAT_RESULTS_MEETING_CHAT_DEAD_LETTER: &str =
    "assistant:chat_results:meeting-chat-dead-letter";
/// Hash of retry counters (not a stream, kept here with its siblings).
pub const ASSISTANT_CHAT_RESULTS_MEETING_CHAT_RETRIES: &str =
    "assistant:chat_results:meeting-chat-retries";

/// Per-meeting copy of a stream: `<stream>:{meetingId}` (spec 08 §1).
pub fn per_meeting(stream: &str, meeting: RoomId) -> String {
    format!("{stream}:{meeting}")
}

/// Dead-letter stream of the transcript persistence consumer (spec 12 §6.1).
pub fn transcript_dead_letter(stream: &str) -> String {
    format!("{stream}:transcript-persistence:dead-letter")
}

/// Consumer group names.
pub mod groups {
    // spec: 10 §5.3
    pub const TRANSLATION_ROOM_BACKEND: &str = "translationRoom_backend_consumer";
    pub const TRANSLATION_ROOM_RECORDINGS: &str = "translation-room.recordings.v1";
    pub const TRANSLATION_ROOM_WORKSPACE: &str = "translation-room-group";
    pub const TRANSLATION_ROOM_SUMMARY: &str = "translation-room-summary-consumers";
    // spec: 11 §6.3
    pub const MEETING_CHAT: &str = "meeting-chat-consumers";
    // spec: 12 §6.1, §6.2
    pub const TRANSCRIPT_PERSISTENCE: &str = "transcript-persistence";
    pub const TRANSCRIPT_GLOSSARY_INDEX: &str = "transcript-glossary-index";
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names_and_builders() {
        assert_eq!(AUDIO_CHUNKS, "audio:chunks");
        assert_eq!(MEETING_DOMAIN_EVENTS, "meeting:domain-events");
        let id = RoomId::new();
        assert_eq!(per_meeting(STT_RESULTS, id), format!("stt:results:{id}"));
        assert_eq!(
            transcript_dead_letter(STT_RESULTS),
            "stt:results:transcript-persistence:dead-letter"
        );
    }
}
