//! Owned, flat, serde-friendly types that cross the public (and future UniFFI) boundary.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Audio channel. `Me` = microphone, `Others` = meeting-app audio (process tap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Channel {
    Me,
    Others,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Me, Channel::Others];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Me => "me",
            Channel::Others => "others",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Channel::Me => 0,
            Channel::Others => 1,
        }
    }

    pub fn other(self) -> Channel {
        match self {
            Channel::Me => Channel::Others,
            Channel::Others => Channel::Me,
        }
    }
}

/// One transcript segment, as posted to the server. Wire form is camelCase; `likely_echo` and
/// `utterance_seq` are local-only (never serialized).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SegmentRecord {
    /// uuid v7, assigned once when the segment is created: retries are idempotent.
    pub client_segment_id: String,
    pub channel: Channel,
    pub text: String,
    pub language: String,
    /// Offset from session start, milliseconds.
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub is_final: bool,
    /// `me` utterance judged to be speaker bleed of `others` (echo guard). Local only.
    #[serde(skip)]
    pub likely_echo: bool,
    /// Per-channel utterance counter (from 0). Local only.
    #[serde(skip)]
    pub utterance_seq: u64,
}

impl SegmentRecord {
    pub fn new_id() -> String {
        Uuid::now_v7().to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum StatusKind {
    Started,
    Stopped,
    SpeechStart,
    SpeechEnd,
    SttConnected,
    SttUnavailable,
    SinkOnline,
    SinkOffline,
    /// Audio chunks were dropped (drop-oldest) since the last status; `detail` has the count.
    AudioDropped,
}

/// Events for the overlay / UI.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum CopilotEvent {
    /// Accumulated text of the utterance in progress (empty text = cleared).
    Partial { channel: Channel, text: String },
    /// A transcript segment (early sentences arrive before the utterance is complete).
    Segment { segment: SegmentRecord },
    Status {
        channel: Option<Channel>,
        kind: StatusKind,
        detail: Option<String>,
    },
    Error {
        channel: Option<Channel>,
        code: String,
        message: String,
    },
    /// Throttled input level (RMS of the audio since the previous `Levels` of the channel).
    Levels { channel: Channel, rms: f32 },
}

/// Result of `push_audio` (never an error: the audio path does not fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum PushStatus {
    Accepted,
    /// Accepted after evicting the oldest queued chunk (counted in stats).
    DroppedOldest,
    /// Engine stopped; audio discarded.
    Closed,
    /// Empty input or sample rate 0.
    Ignored,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ChannelStats {
    pub chunks_pushed: u64,
    pub chunks_dropped: u64,
    pub utterances: u64,
    pub segments: u64,
    pub echo_marked: u64,
    pub stt_errors: u64,
    pub stt_sessions_opened: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SinkStats {
    pub submitted: u64,
    pub delivered: u64,
    pub retries: u64,
    pub rejected: u64,
    pub dropped_overflow: u64,
    pub buffered: u64,
    pub offline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct EngineStats {
    pub me: ChannelStats,
    pub others: ChannelStats,
    pub sink: SinkStats,
    pub events_dropped: u64,
}
