use serde::Deserialize;
use warptalk_ingress::EndpointerConfig;

use crate::error::PipelineError;
use crate::stt::SttMode;

/// Runtime-tunable pipeline settings. Hot-reloadable through
/// [`crate::RoomPipeline::update_config`]; a speaker picks the new value up at the start of its
/// **next utterance** (never mid-utterance). Exceptions fixed at creation: `segment_channel_capacity`
/// (room level) and `frame_queue_capacity` (applies to speakers added afterwards).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct PipelineConfig {
    pub endpointer: EndpointerConfig,
    /// Stream ungated live frames to STT while the speaker talks (spec 08 flash mode).
    /// Only effective with `stt_mode = realtime`.
    pub flash_mode: bool,
    pub stt_mode: SttMode,
    /// Per-speaker input queue, in 32 ms frames. On overflow the oldest frame is dropped.
    pub frame_queue_capacity: usize,
    /// Per-speaker STT -> relay channel.
    pub stt_event_capacity: usize,
    /// Room output channel (bounded: backpressure towards STT, never towards audio).
    pub segment_channel_capacity: usize,
    /// Claim an STT session (warm socket) when the speaker joins instead of at first speech.
    pub eager_session_open: bool,
    /// Upper bound for draining one speaker on remove/shutdown.
    pub shutdown_grace_ms: u64,
    /// Also emit an `SttSegment::utterance_end` marker (empty text) when a turn that produced
    /// text is complete. Off by default (plain transcript consumers never see it). Applies to
    /// speakers added afterwards.
    pub emit_utterance_end: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            endpointer: EndpointerConfig::default(),
            flash_mode: true,
            stt_mode: SttMode::Realtime,
            frame_queue_capacity: 256, // ~8 s of audio
            stt_event_capacity: 64,
            segment_channel_capacity: 64,
            eager_session_open: true,
            shutdown_grace_ms: 5_000,
            emit_utterance_end: false,
        }
    }
}

impl PipelineConfig {
    pub fn validate(&self) -> Result<(), PipelineError> {
        self.endpointer
            .validate()
            .map_err(PipelineError::InvalidConfig)?;
        for (name, v) in [
            ("frame_queue_capacity", self.frame_queue_capacity),
            ("stt_event_capacity", self.stt_event_capacity),
            ("segment_channel_capacity", self.segment_channel_capacity),
        ] {
            if v == 0 {
                return Err(PipelineError::InvalidConfig(format!("{name} must be > 0")));
            }
        }
        Ok(())
    }

    /// Whether live frames are streamed to STT for this config.
    pub fn streams_live(&self) -> bool {
        self.flash_mode && self.stt_mode == SttMode::Realtime
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_for_missing_fields() {
        let cfg: PipelineConfig = serde_json_from(r#"{"flash_mode": false}"#);
        assert!(!cfg.flash_mode);
        assert_eq!(cfg.frame_queue_capacity, 256);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn nested_endpointer_and_mode() {
        let cfg: PipelineConfig = serde_json_from(
            r#"{"stt_mode":"closed_chunk","endpointer":{"vad_silence_hangover_ms":400}}"#,
        );
        assert_eq!(cfg.stt_mode, SttMode::ClosedChunk);
        assert_eq!(cfg.endpointer.vad_silence_hangover_ms, 400);
        assert!(!cfg.streams_live());
    }

    #[test]
    fn rejects_zero_capacity_and_bad_endpointer() {
        let mut cfg = PipelineConfig {
            frame_queue_capacity: 0,
            ..PipelineConfig::default()
        };
        assert!(cfg.validate().is_err());
        cfg.frame_queue_capacity = 8;
        cfg.endpointer.chunk_duration_ms = 10;
        assert!(cfg.validate().is_err());
    }

    fn serde_json_from(s: &str) -> PipelineConfig {
        serde_json::from_str(s).unwrap()
    }
}
