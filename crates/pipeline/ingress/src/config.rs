use serde::Deserialize;

/// Every endpointing knob (docs/spec/08 section 6). Loaded from runtime/platform settings;
/// call sites must never hardcode values. Missing fields take the defaults below.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct EndpointerConfig {
    /// Speech-entry threshold on the VAD probability.
    pub vad_threshold: f32,
    /// Speech-exit threshold (hysteresis). Clamped to `vad_threshold` at use.
    pub vad_release_threshold: f32,
    /// Audio before speech onset prepended to the utterance.
    pub vad_pre_speech_ms: u32,
    /// Minimum SPEECH (not padding) for an utterance to be published.
    pub vad_min_speech_ms: u32,
    /// Ordinary end-of-sentence hangover (middle rung).
    pub vad_silence_hangover_ms: u32,
    /// Hangover once the turn has `vad_seek_boundary_after_ms` of speech (boundary seeking).
    pub vad_seek_hangover_ms: u32,
    /// Hangover for a turn with under `vad_short_turn_speech_ms` of speech.
    pub vad_short_turn_hangover_ms: u32,
    /// Speech-so-far at which the seek rung starts.
    pub vad_seek_boundary_after_ms: u32,
    /// Speech-so-far under which the short-turn rung applies.
    pub vad_short_turn_speech_ms: u32,
    /// Hard cap on one chunk (padding included). Platform range 2000..=15000.
    pub chunk_duration_ms: u32,
    /// Absolute energy floor on SPEECH RMS (scaled by sqrt of speech share).
    pub energy_floor_rms: f32,
    /// Speaker-relative floor as a fraction of the speaker's baseline (0 disables).
    pub energy_relative_ratio: f32,
    /// Chunks that cleared the absolute floor before a baseline exists.
    pub energy_baseline_min_chunks: usize,
    /// Number of recent chunks the baseline median is taken over.
    pub energy_baseline_window: usize,
}

impl Default for EndpointerConfig {
    fn default() -> Self {
        Self {
            vad_threshold: 0.35,
            vad_release_threshold: 0.20,
            vad_pre_speech_ms: 384,
            vad_min_speech_ms: 288,
            vad_silence_hangover_ms: 576,
            vad_seek_hangover_ms: 250,
            vad_short_turn_hangover_ms: 864,
            vad_seek_boundary_after_ms: 4000,
            vad_short_turn_speech_ms: 1500,
            chunk_duration_ms: 6000,
            energy_floor_rms: 0.02,
            energy_relative_ratio: 0.4,
            energy_baseline_min_chunks: 3,
            energy_baseline_window: 8,
        }
    }
}

impl EndpointerConfig {
    /// Exit threshold actually used: never above the entry threshold.
    pub fn effective_release_threshold(&self) -> f32 {
        self.vad_release_threshold.min(self.vad_threshold)
    }

    pub fn validate(&self) -> Result<(), String> {
        fn unit(name: &str, v: f32) -> Result<(), String> {
            if v.is_finite() && v > 0.0 && v <= 1.0 {
                Ok(())
            } else {
                Err(format!("{name} must be in (0, 1], got {v}"))
            }
        }
        unit("vad_threshold", self.vad_threshold)?;
        unit("vad_release_threshold", self.vad_release_threshold)?;
        if !(2000..=15000).contains(&self.chunk_duration_ms) {
            return Err(format!(
                "chunk_duration_ms must be in 2000..=15000, got {}",
                self.chunk_duration_ms
            ));
        }
        if self.vad_pre_speech_ms > 2000 {
            return Err("vad_pre_speech_ms must be <= 2000".into());
        }
        if self.vad_min_speech_ms == 0 || self.vad_min_speech_ms >= self.chunk_duration_ms {
            return Err("vad_min_speech_ms must be in 1..chunk_duration_ms".into());
        }
        if self.vad_seek_hangover_ms == 0 {
            return Err("vad_seek_hangover_ms must be > 0".into());
        }
        if !(self.vad_seek_hangover_ms <= self.vad_silence_hangover_ms
            && self.vad_silence_hangover_ms <= self.vad_short_turn_hangover_ms)
        {
            return Err("hangover ladder must satisfy seek <= silence <= short_turn".into());
        }
        if self.vad_short_turn_hangover_ms >= self.chunk_duration_ms {
            return Err("vad_short_turn_hangover_ms must be < chunk_duration_ms".into());
        }
        if self.vad_short_turn_speech_ms > self.vad_seek_boundary_after_ms {
            return Err("vad_short_turn_speech_ms must be <= vad_seek_boundary_after_ms".into());
        }
        // seek_after >= chunk cap is allowed: seeking then simply never triggers.
        if !self.energy_floor_rms.is_finite() || !(0.0..=1.0).contains(&self.energy_floor_rms) {
            return Err("energy_floor_rms must be in [0, 1]".into());
        }
        if !self.energy_relative_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.energy_relative_ratio)
        {
            return Err("energy_relative_ratio must be in [0, 1]".into());
        }
        if self.energy_baseline_window == 0 || self.energy_baseline_min_chunks == 0 {
            return Err(
                "energy_baseline_window and energy_baseline_min_chunks must be >= 1".into(),
            );
        }
        if self.energy_baseline_min_chunks > self.energy_baseline_window {
            return Err("energy_baseline_min_chunks must be <= energy_baseline_window".into());
        }
        Ok(())
    }
}
