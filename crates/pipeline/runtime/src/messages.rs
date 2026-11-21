//! Messages flowing between stages. Every message carries `Instant` stamps taken on the
//! monotonic clock so per-stage latency is a subtraction, never a clock-sync problem.

use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;
use warptalk_audio::Frame;
use warptalk_ingress::EndReason;

pub type RoomId = String;
pub type ParticipantId = String;

/// One 32 ms mono 16 kHz frame plus the instant it reached the process.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub frame: Frame,
    pub arrived_at: Instant,
}

impl AudioFrame {
    /// Stamps the frame with "now". Call this as close to the audio source as possible.
    pub fn now(frame: Frame) -> Self {
        Self {
            frame,
            arrived_at: Instant::now(),
        }
    }
}

/// Latency stamps of one turn. Order for a closed-chunk turn:
/// `audio_arrived <= speech_end <= stt_commit <= stt_first_text <= stt_final`.
///
/// In flash mode an early sentence is transcribed while the speaker is still talking, so
/// `stt_first_text` may precede `stt_commit`; the `stt_first_text` metric saturates at 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamps {
    /// Arrival of the frame that opened the turn (speech start).
    pub audio_arrived: Instant,
    /// Arrival of the last speech frame.
    pub speech_end: Option<Instant>,
    /// Just before the STT commit was issued.
    pub stt_commit: Option<Instant>,
    /// First text (early sentence or final) reached the runtime.
    pub stt_first_text: Option<Instant>,
    /// The transcript for the turn completed.
    pub stt_final: Option<Instant>,
}

impl Stamps {
    pub fn new(audio_arrived: Instant) -> Self {
        Self {
            audio_arrived,
            speech_end: None,
            stt_commit: None,
            stt_first_text: None,
            stt_final: None,
        }
    }
}

/// A closed utterance, as handed to STT at commit.
#[derive(Debug, Clone)]
pub struct Utterance {
    pub room: RoomId,
    pub speaker: ParticipantId,
    /// Per-speaker utterance counter, from 0.
    pub seq: u64,
    pub lang: Option<String>,
    /// Gated, trimmed 16 kHz mono audio including pre-speech padding.
    pub audio: Arc<[f32]>,
    pub speech_ms: u32,
    pub total_ms: u32,
    /// Stream-time delay from the last speech frame to the endpoint decision (the hangover).
    pub hangover_ms: u32,
    pub reason: EndReason,
    pub stamps: Stamps,
}

/// One piece of transcript. A turn yields zero or more early segments (`is_early`, flash mode,
/// append-only) and at most one remainder (`is_early == false`).
#[derive(Debug, Clone)]
pub struct SttSegment {
    pub segment_id: Uuid,
    pub room: RoomId,
    pub speaker: ParticipantId,
    /// Per-speaker segment counter assigned by the runtime in arrival order, from 0.
    pub seq: u64,
    /// Utterance this segment belongs to (assigned by the runtime).
    pub utterance_seq: u64,
    pub text: String,
    pub lang: String,
    /// Average token probability proxy (avg logprob); `None` when the model gave none.
    pub confidence: Option<f32>,
    pub is_early: bool,
    pub stamps: Stamps,
}

impl SttSegment {
    /// For STT implementations: identity fields, `seq`, `utterance_seq` and `stamps` are
    /// overwritten by the runtime relay, so a session only supplies the content.
    pub fn new(
        text: impl Into<String>,
        lang: impl Into<String>,
        confidence: Option<f32>,
        is_early: bool,
    ) -> Self {
        Self {
            segment_id: Uuid::now_v7(),
            room: String::new(),
            speaker: String::new(),
            seq: 0,
            utterance_seq: 0,
            text: text.into(),
            lang: lang.into(),
            confidence,
            is_early,
            stamps: Stamps::new(Instant::now()),
        }
    }
}
