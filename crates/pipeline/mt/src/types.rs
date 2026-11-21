use std::time::Instant;

use uuid::Uuid;
use warptalk_contracts::TranslateResult;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::Stamps;

/// Latency stamps through MT: the STT stamps plus the two MT ones.
/// Order: `stt_* <= mt_start <= mt_first_token <= mt_done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtStamps {
    pub stt: Stamps,
    /// The segment reached the MT stage.
    pub mt_start: Instant,
    /// First text from the model (Realtime: first delta). For passthrough, equals `mt_done`.
    pub mt_first_token: Option<Instant>,
    pub mt_done: Option<Instant>,
}

impl MtStamps {
    pub fn mt_first_token_ms(&self) -> Option<u64> {
        self.mt_first_token
            .map(|t| t.saturating_duration_since(self.mt_start).as_millis() as u64)
    }
    pub fn mt_done_ms(&self) -> Option<u64> {
        self.mt_done
            .map(|t| t.saturating_duration_since(self.mt_start).as_millis() as u64)
    }
}

/// One translated (or polished same-language) sentence for one target language.
#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    /// The STT segment this sentence came from.
    pub segment_id: Uuid,
    /// Sentence index inside the segment, from 0.
    pub chunk_idx: u32,
    pub is_final_chunk: bool,
    pub room: String,
    pub speaker: String,
    /// Per-speaker segment counter (from STT).
    pub seq: u64,
    pub utterance_seq: u64,
    pub source_lang: String,
    pub target_lang: String,
    pub source_text: String,
    pub text: String,
    /// Model that produced `text`; `"passthrough"` / `"passthrough+polish"` for same-language.
    pub model: String,
    /// Same language as the speaker: captions only, TTS must not dub it.
    pub passthrough: bool,
    pub is_early: bool,
    pub confidence: Option<f32>,
    /// Estimated source speech duration of THIS sentence (for isochrony), when it can be known:
    /// a whole-turn segment with no early sentences, apportioned by characters.
    pub source_ms: Option<u32>,
    pub stamps: MtStamps,
}

impl Translation {
    /// Id as published on the wire: one per (segment, target language, sentence).
    pub fn wire_segment_id(&self) -> String {
        format!(
            "{}-{}-c{}",
            self.segment_id, self.target_lang, self.chunk_idx
        )
    }

    pub fn latency_ms(&self) -> Option<i64> {
        self.stamps.mt_done_ms().map(|v| v as i64)
    }

    pub fn to_contract(&self) -> TranslateResult {
        TranslateResult {
            segment_id: self.wire_segment_id(),
            target_lang: self.target_lang.clone(),
            translated_text: self.text.clone(),
            translator_model: Some(self.model.clone()),
            latency_ms: self.latency_ms(),
            is_retranslated: false,
            previous_translation_content_id: None,
            source_stt_confidence: self.confidence.map(f64::from),
            meeting_id: self.room.parse::<RoomId>().ok(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// `__MEETING_END__`-style control segment: never translated.
    ControlMarker,
    Empty,
    NotStarted,
    CreditsExhausted,
    Paused,
    NoTargets,
}

/// Side-channel events (best effort, never block the pipeline).
#[derive(Debug, Clone)]
pub enum MtEvent {
    /// Translation of a sentence failed after the chat fallback; the source transcript is still
    /// delivered by the caption path (translation is additive, spec 13 section 2.5).
    Failed {
        room: String,
        speaker: String,
        segment_id: Uuid,
        chunk_idx: u32,
        target_lang: String,
        error: String,
    },
    Skipped {
        room: String,
        speaker: String,
        segment_id: Uuid,
        reason: SkipReason,
    },
    /// A later segment went out before its predecessor finished (bounded wait elapsed).
    OrderWaitExceeded {
        room: String,
        speaker: String,
        segment_id: Uuid,
        target_lang: String,
    },
}

/// A finished sentence from an early STT delta; translated ahead into a cache, never published
/// (spec 08 section 4 item 5).
#[derive(Debug, Clone)]
pub struct SpeculativeHint {
    pub room: String,
    pub speaker: String,
    pub text: String,
    pub lang: String,
}
