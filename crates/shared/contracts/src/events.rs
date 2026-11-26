//! Payload types carried on the streams/channels. JSON is snake_case unless noted.
//! No IO. Numeric millisecond fields are `i64`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{ParticipantId, RoomId, UserId, WorkspaceId};

/// Text of the end-of-meeting control segment published on `stt:results` (spec: 11 §6.3).
pub const MEETING_END_MARKER: &str = "__MEETING_END__";
/// `speaker_id` / `language` used by system segments (spec: 11 §6.3).
pub const SYSTEM_SPEAKER: &str = "system";

/// True for control markers matching `^__[A-Z0-9_]+__$`.
pub fn is_control_marker(text: &str) -> bool {
    let Some(inner) = text.strip_prefix("__").and_then(|t| t.strip_suffix("__")) else {
        return false;
    };
    !inner.is_empty()
        && inner
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// STT segment on `stt:results` (spec: 11 §6.3, 12 §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttResult {
    // spec: 12 §6.1 (segment ids are opaque strings on the wire)
    pub segment_id: String,
    pub meeting_id: RoomId,
    pub speaker_id: String,
    pub text: String,
    pub language: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    // spec: 11 §6.3
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i64>,
    #[serde(default)]
    pub is_final_chunk: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
    // spec: 12 §6.1 (clean_flags, far_speaker_*). The pipeline also reports the speaking
    // participant's display name here (`far_speaker_source = "participant"`, confidence 1.0):
    // the transcript stores `far_speaker_name` as the segment's speaker name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_flags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_speaker_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_speaker_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_speaker_confidence: Option<f64>,
    /// Capture channel of a copilot segment: `"me"` (the user's own microphone) or `"others"`
    /// (meeting audio). Absent for every other producer. Producer: copilot ingest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

impl SttResult {
    /// Build the `__MEETING_END__` system marker segment.
    pub fn meeting_end(segment_id: String, meeting_id: RoomId, now_ms: i64) -> Self {
        Self {
            segment_id,
            meeting_id,
            speaker_id: SYSTEM_SPEAKER.into(),
            text: MEETING_END_MARKER.into(),
            language: SYSTEM_SPEAKER.into(),
            start_ms: now_ms,
            end_ms: now_ms,
            anchor_ms: None,
            confidence: Some(1.0),
            chunk_index: Some(0),
            is_final_chunk: true,
            timestamp_ms: Some(now_ms),
            clean_flags: None,
            far_speaker_name: None,
            far_speaker_source: None,
            far_speaker_confidence: None,
            channel: None,
        }
    }

    pub fn is_control_marker(&self) -> bool {
        is_control_marker(&self.text)
    }
}

/// Translation on `translate:results` / `translate:backfill_results` (spec: 12 §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslateResult {
    pub segment_id: String,
    pub target_lang: String,
    pub translated_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translator_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub is_retranslated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_translation_content_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_stt_confidence: Option<f64>,
    // spec: 08 §1 (stream is per meeting; id kept in payload for routing)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<RoomId>,
}

/// Dubbing metadata on `tts:results`. Audio bytes travel on the LiveKit track, never here (Rust design D2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsResult {
    pub segment_id: String,
    pub target_lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_voice_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<RoomId>,
}

/// Clean sentence on `transcript:clean` (spec: 12 §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptClean {
    pub sentence_id: String,
    pub revision: i64,
    pub clean_text: String,
    pub segment_ids: Vec<String>,
    pub language: String,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
    pub meeting_id: RoomId,
}

/// Envelope for `meeting:domain-events` and meeting pub/sub events (spec: 11 §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventEnvelope {
    pub event_id: Uuid,
    pub event_type: String,
    pub schema_version: u32,
    pub producer: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

/// Producer name of meeting events (spec: 11 §6.2).
pub const MEETING_PRODUCER: &str = "meeting-service";
pub const EVENT_MEETING_STARTED: &str = "meeting.started";
pub const EVENT_RECORDING_STARTED: &str = "meeting.recording_started";
pub const EVENT_RECORDING_COMPLETED: &str = "meeting.recording_completed";
pub const EVENT_RECORDING_FAILED: &str = "meeting.recording_failed";

/// Payload of `meeting.recording_completed` (spec: 11 §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingCompleted {
    pub translation_room_id: RoomId,
    pub egress_id: String,
    pub file_url: String,
    pub file_format: String,
    pub file_size_bytes: i64,
    pub contains_raw_audio: bool,
    pub contains_raw_video: bool,
}

/// Cached under the `audio_routes` key and sent as AUDIO_ROUTES_UPDATED (spec: 10 §3.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRoutesSnapshot {
    pub generated_at: DateTime<Utc>,
    pub room_status: String, // spec: 10 §3.7 (status enum not listed here)
    pub translation_active: bool,
    pub room_languages: Vec<String>,
    pub routes: Vec<AudioRoute>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRoute {
    pub route_id: Uuid,
    pub source_participant_id: ParticipantId,
    pub target_participant_id: ParticipantId,
    /// User behind the source participant. The media layer (LiveKit identity) and the voice
    /// consent key are per user, the route is per participant; absent in old snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_user_id: Option<UserId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_user_id: Option<UserId>,
    /// Display name of the source participant (room participants table): the pipeline puts it
    /// on captions (`speakerName`) and `stt:results` (`speaker_name`). Absent in old snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_display_name: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub voice_clone_enabled: bool,
    pub status: RouteStatus,
}

/// Audio route statuses (spec: 10 §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RouteStatus {
    Pending,
    Ready,
    Broadcasting,
    CaptionOnly,
    StandardVoice,
    VoiceDelayed,
    TranslationDelayed,
    SpeechDelayed,
    Paused,
    Ending,
    SavingOutputs,
    Completed,
}

/// Participant entry of `RoomStarted` (spec: 10 §5.2; camelCase as listed there).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomStartedParticipant {
    pub user_id: Uuid,
    pub display_name: String,
    pub speak_language: String,
    pub listen_language: String,
    pub is_muted: bool,
    pub joined_at: DateTime<Utc>,
}

/// State of `RoomStarted` (spec: 10 §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomStartedState {
    pub room_id: RoomId,
    pub code: String,
    pub status: String,
    pub participants: Vec<RoomStartedParticipant>,
}

/// Command on `warptalk:translation-room:commands` (spec: 10 §5.2, 11 §6.3).
/// Wire shape `{Command, RoomId, ...}`; field names PascalCase (spec: 11 §6.3 `Locked`, `Recording`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "Command", rename_all_fields = "PascalCase")]
pub enum RoomCommand {
    RoomStarted {
        room_id: RoomId,
        state: RoomStartedState,
    },
    RoomEnded {
        room_id: RoomId,
    },
    CancelRoom {
        room_id: RoomId,
    },
    Kick {
        room_id: RoomId,
        user_id: Option<Uuid>, // spec: 10 §5.2 (fields unlisted)
    },
    ParticipantAdmitted {
        room_id: RoomId,
        user_id: Option<Uuid>,
    },
    ParticipantRejected {
        room_id: RoomId,
        user_id: Option<Uuid>,
    },
    ParticipantWaiting {
        room_id: RoomId,
        user_id: Option<Uuid>,
    },
    TranslationStopped {
        room_id: RoomId,
    },
    RoomLanguagesChanged {
        room_id: RoomId,
        languages: Option<Vec<String>>,
    },
    RoomLockChanged {
        room_id: RoomId,
        locked: bool,
    },
    RecordingStateChanged {
        room_id: RoomId,
        recording: bool,
    },
    // spec: 11 §6.3 ("host transfer", name and fields guessed)
    HostTransferred {
        room_id: RoomId,
        new_host_user_id: Option<Uuid>,
    },
}

/// Fields of `translationRoom:system_events` entries (spec: 10 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationRoomSystemEvent {
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<Uuid>,
    pub room_id: RoomId,
    pub payload: Value,
}

/// Workspace events (spec: 10 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event_type")]
pub enum WorkspaceEvent {
    WorkspaceDeleted { workspace_id: WorkspaceId },
    MemberRemoved { user_id: Uuid },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn rt<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(v: T) -> Value {
        let j = serde_json::to_value(&v).unwrap();
        let back: T = serde_json::from_value(j.clone()).unwrap();
        assert_eq!(v, back);
        j
    }

    #[test]
    fn control_markers() {
        assert!(is_control_marker("__MEETING_END__"));
        assert!(is_control_marker("__A1__"));
        assert!(!is_control_marker("__meeting_end__"));
        assert!(!is_control_marker("____"));
        assert!(!is_control_marker("__"));
        assert!(!is_control_marker("MEETING_END"));
        assert!(!is_control_marker("__A B__"));
        assert!(!is_control_marker("__A__ "));
        assert!(!is_control_marker(""));
    }

    #[test]
    fn stt_roundtrip_and_marker() {
        let m = SttResult::meeting_end("s1".into(), RoomId::new(), 5);
        assert!(m.is_control_marker());
        let j = rt(m);
        assert_eq!(j["speaker_id"], "system");
        assert_eq!(j["text"], MEETING_END_MARKER);
    }

    #[test]
    fn translate_tts_clean_roundtrip() {
        rt(TranslateResult {
            segment_id: "s".into(),
            target_lang: "en".into(),
            translated_text: "hi".into(),
            translator_model: Some("m".into()),
            latency_ms: Some(120),
            is_retranslated: false,
            previous_translation_content_id: None,
            source_stt_confidence: Some(0.9),
            meeting_id: None,
        });
        let j = rt(TtsResult {
            segment_id: "s".into(),
            target_lang: "en".into(),
            voice_type: Some("clone".into()),
            provider_voice_id: None,
            clone_provider: None,
            anchor_provider: None,
            duration_ms: Some(900),
            meeting_id: Some(RoomId::new()),
        });
        assert!(j.get("audio").is_none());
        rt(TranscriptClean {
            sentence_id: "x".into(),
            revision: 2,
            clean_text: "t".into(),
            segment_ids: vec!["a".into()],
            language: "vi".into(),
            flags: vec![],
            source: None,
            timestamp_ms: Some(1),
            meeting_id: RoomId::new(),
        });
    }

    #[test]
    fn envelope_and_recording() {
        let rec = RecordingCompleted {
            translation_room_id: RoomId::new(),
            egress_id: "e".into(),
            file_url: "u".into(),
            file_format: "mp4".into(),
            file_size_bytes: 10,
            contains_raw_audio: true,
            contains_raw_video: false,
        };
        let env = DomainEventEnvelope {
            event_id: Uuid::now_v7(),
            event_type: EVENT_RECORDING_COMPLETED.into(),
            schema_version: 1,
            producer: MEETING_PRODUCER.into(),
            occurred_at: Utc::now(),
            payload: serde_json::to_value(&rec).unwrap(),
        };
        rt(env.clone());
        let back: RecordingCompleted = serde_json::from_value(env.payload).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn routes_snapshot() {
        let s = AudioRoutesSnapshot {
            generated_at: Utc::now(),
            room_status: "IN_PROGRESS".into(),
            translation_active: true,
            room_languages: vec!["en".into(), "vi".into()],
            routes: vec![AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: ParticipantId::new(),
                target_participant_id: ParticipantId::new(),
                source_user_id: Some(UserId::new()),
                target_user_id: None,
                source_display_name: Some("Ann".into()),
                source_language: "vi".into(),
                target_language: "en".into(),
                voice_clone_enabled: false,
                status: RouteStatus::SavingOutputs,
            }],
        };
        let j = rt(s);
        assert_eq!(j["routes"][0]["status"], "SAVING_OUTPUTS");
        assert!(j["routes"][0]["source_user_id"].is_string());
        assert_eq!(j["routes"][0]["source_display_name"], "Ann");
        assert!(j["routes"][0].get("target_user_id").is_none());
        // Old snapshots without the user ids still parse.
        let mut old = j.clone();
        old["routes"][0]
            .as_object_mut()
            .unwrap()
            .remove("source_user_id");
        old["routes"][0]
            .as_object_mut()
            .unwrap()
            .remove("source_display_name");
        let back: AudioRoutesSnapshot = serde_json::from_value(old).unwrap();
        assert_eq!(back.routes[0].source_user_id, None);
        assert_eq!(back.routes[0].source_display_name, None);
        assert_eq!(
            serde_json::to_value(RouteStatus::CaptionOnly).unwrap(),
            "CAPTION_ONLY"
        );
    }

    #[test]
    fn room_commands() {
        let id = RoomId::new();
        let j = rt(RoomCommand::RoomLockChanged {
            room_id: id,
            locked: true,
        });
        assert_eq!(j["Command"], "RoomLockChanged");
        assert_eq!(j["Locked"], true);
        assert_eq!(j["RoomId"], serde_json::to_value(id).unwrap());
        rt(RoomCommand::RoomStarted {
            room_id: id,
            state: RoomStartedState {
                room_id: id,
                code: "abc".into(),
                status: "IN_PROGRESS".into(),
                participants: vec![RoomStartedParticipant {
                    user_id: Uuid::now_v7(),
                    display_name: "A".into(),
                    speak_language: "vi".into(),
                    listen_language: "en".into(),
                    is_muted: false,
                    joined_at: Utc::now(),
                }],
            },
        });
        rt(RoomCommand::Kick {
            room_id: id,
            user_id: None,
        });
        rt(RoomCommand::RoomLanguagesChanged {
            room_id: id,
            languages: Some(vec!["en".into()]),
        });
        rt(RoomCommand::RecordingStateChanged {
            room_id: id,
            recording: false,
        });
        rt(RoomCommand::HostTransferred {
            room_id: id,
            new_host_user_id: None,
        });
    }

    #[test]
    fn system_and_workspace_events() {
        rt(TranslationRoomSystemEvent {
            event_type: "config_ready".into(),
            route_id: None,
            room_id: RoomId::new(),
            payload: serde_json::json!({"a":1}),
        });
        rt(WorkspaceEvent::WorkspaceDeleted {
            workspace_id: WorkspaceId::new(),
        });
        rt(WorkspaceEvent::MemberRemoved {
            user_id: Uuid::now_v7(),
        });
    }
}
