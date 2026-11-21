use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;
use warptalk_contracts::events::TtsResult;
use warptalk_kernel::RoomId;
use warptalk_mt::MtStamps;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceMode {
    Cloned,
    Standard,
}

impl VoiceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloned => "cloned",
            Self::Standard => "standard",
        }
    }
}

/// Where a voice came from (spec 13 section 4 order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSource {
    InMeetingClone,
    CarriedClone,
    ChosenDub,
    Catalog,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    pub voice_id: String,
    pub mode: VoiceMode,
    pub source: VoiceSource,
}

/// Degradation ladder (arch 5.3): clone -> standard voice -> text only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RouteTier {
    Clone = 2,
    Standard = 1,
    TextOnly = 0,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TtsStamps {
    pub mt: MtStamps,
    /// The worker started on this sentence.
    pub tts_start: Instant,
    pub tts_first_audio: Option<Instant>,
}

/// One piece of dubbed audio, streamed as the vendor produces it (never a whole sentence
/// buffered). The sentence ends with a chunk whose `is_last` is true; that chunk may carry zero
/// samples (the stream finishes after the last audio was already forwarded). One synthesis
/// serves every listener of (speaker, language, voice mode): fan out downstream.
#[derive(Debug, Clone, PartialEq)]
pub struct DubChunk {
    pub segment_id: Uuid,
    pub chunk_idx: u32,
    pub room: String,
    pub speaker: String,
    pub target_lang: String,
    pub voice: VoiceInfo,
    /// Mono f32 PCM at `sample_rate`.
    pub pcm: Arc<[f32]>,
    pub sample_rate: u32,
    /// Per-sentence chunk counter from 0.
    pub seq: u32,
    pub is_last: bool,
    pub cache_hit: bool,
    pub stamps: TtsStamps,
}

#[derive(Debug, Clone)]
pub enum TtsEvent {
    RouteDegraded {
        room: String,
        speaker: String,
        target_lang: String,
        from: RouteTier,
        to: RouteTier,
        reason: String,
    },
    SentenceDone {
        segment_id: Uuid,
        chunk_idx: u32,
        room: String,
        speaker: String,
        target_lang: String,
        voice: VoiceInfo,
        duration_ms: u32,
        cache_hit: bool,
        slot_wait_ms: u64,
        continued: bool,
    },
    /// Given up: the line is parked with the reason (partially spoken lines are never replayed).
    DeadLetter {
        segment_id: Uuid,
        chunk_idx: u32,
        room: String,
        speaker: String,
        target_lang: String,
        attempts: u32,
        reason: String,
        error: String,
    },
}

impl TtsEvent {
    /// `tts:results` metadata for a finished sentence.
    pub fn to_contract(&self) -> Option<TtsResult> {
        let Self::SentenceDone {
            segment_id,
            chunk_idx,
            room,
            target_lang,
            voice,
            duration_ms,
            ..
        } = self
        else {
            return None;
        };
        Some(TtsResult {
            segment_id: format!("{segment_id}-{target_lang}-c{chunk_idx}"),
            target_lang: target_lang.clone(),
            voice_type: Some(voice.mode.as_str().into()),
            provider_voice_id: Some(voice.voice_id.clone()),
            clone_provider: (voice.mode == VoiceMode::Cloned).then(|| "cartesia".into()),
            anchor_provider: Some("cartesia".into()),
            duration_ms: Some(i64::from(*duration_ms)),
            meeting_id: room.parse::<RoomId>().ok(),
        })
    }
}
