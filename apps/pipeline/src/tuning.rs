//! Latency flags and the extra keys of the `pipeline` settings namespace.
//!
//! The namespace document is the typed `PipelineSettings` (VAD knobs and friends) plus three
//! more top-level keys read here: `mt` ([`MtConfig`]), `tts` ([`TtsConfig`]) and `latency`
//! ([`LatencyTuning`]). Unknown keys are ignored by each reader, so both views of the same JSON
//! document coexist. Every key has a default, so a partial document is valid, and the whole
//! thing hot-reloads through the settings watch.

use serde::Deserialize;
use warptalk_mt::MtConfig;
use warptalk_settings::Validate;
use warptalk_tts::TtsConfig;

/// Switches of the latency optimizations (docs/perf/README.md has the measurement behind each
/// default).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct LatencyTuning {
    /// (a) Early endpoint: once the streaming STT produced a sentence terminator for the turn
    /// in progress, the VAD hangover of that turn is cut to this many ms. 0 disables.
    pub early_endpoint_hangover_ms: u32,
    /// (b) Pipelining: hand every early STT sentence to MT the moment it arrives. When off, the
    /// early sentences of a turn are held and translated together when the turn completes.
    pub early_mt: bool,
    /// (c) Open (claim) the TTS contexts of the target languages when a segment reaches MT,
    /// so a cold socket dial overlaps the translation instead of following it.
    pub tts_prewarm: bool,
    /// A pre-opened context unused for this long is closed.
    pub tts_prewarm_max_age_ms: u64,
    /// (d) Capacity of the bounded channels between the downstream sub-stages (STT -> MT -> TTS
    /// -> publisher). Only backpressure towards STT; audio never waits on them.
    pub stage_channel_capacity: usize,
}

impl Default for LatencyTuning {
    fn default() -> Self {
        Self {
            // Measured (docs/perf): no gain once `early_mt` is on, so off; it helps only
            // deployments that run with `early_mt` off (about -145 ms p50 there).
            early_endpoint_hangover_ms: 0,
            // Measured: speech end -> first dub p50 2361 -> 1407 ms, p95 3080 -> 2020 ms.
            early_mt: true,
            // Measured: TTS first audio p95 700 -> 260 ms, e2e p95 -200 ms on top of `early_mt`.
            tts_prewarm: true,
            tts_prewarm_max_age_ms: 8_000,
            stage_channel_capacity: 64,
        }
    }
}

impl LatencyTuning {
    pub fn validate(&self) -> Result<(), String> {
        if self.early_endpoint_hangover_ms > 576 {
            return Err("early_endpoint_hangover_ms must be <= 576".into());
        }
        if self.stage_channel_capacity == 0 {
            return Err("stage_channel_capacity must be > 0".into());
        }
        Ok(())
    }
}

/// The keys of the `pipeline` namespace that the typed `PipelineSettings` does not carry.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PipelineExtras {
    pub mt: MtConfig,
    pub tts: TtsConfig,
    pub latency: LatencyTuning,
}

impl Validate for PipelineExtras {
    fn validate(&self) -> Result<(), String> {
        self.mt.validate().map_err(|e| e.to_string())?;
        self.tts.validate().map_err(|e| e.to_string())?;
        self.latency.validate()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_is_valid_defaults() {
        let e: PipelineExtras = serde_json::from_str("{}").unwrap();
        assert_eq!(e, PipelineExtras::default());
        e.validate().unwrap();
    }

    #[test]
    fn partial_keys_and_foreign_keys() {
        let e: PipelineExtras = serde_json::from_str(
            r#"{"vad_threshold":0.4,"mt":{"glossary_limit":3},"tts":{"max_concurrency":5},
                "latency":{"early_mt":true}}"#,
        )
        .unwrap();
        assert_eq!(e.mt.glossary_limit, 3);
        assert_eq!(e.tts.max_concurrency, 5);
        assert!(e.latency.early_mt);
        assert_eq!(e.latency.stage_channel_capacity, 64);
        e.validate().unwrap();
    }

    #[test]
    fn rejects_bad_latency() {
        let mut e = PipelineExtras::default();
        e.latency.early_endpoint_hangover_ms = 900;
        assert!(e.validate().is_err());
        e.latency.early_endpoint_hangover_ms = 250;
        e.latency.stage_channel_capacity = 0;
        assert!(e.validate().is_err());
    }
}
