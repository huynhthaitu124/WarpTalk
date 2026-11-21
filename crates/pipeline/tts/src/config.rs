//! All TTS knobs. Stored as JSON in the `pipeline` settings namespace (key `tts`); every field
//! has a default (spec 13 sections 3, 6, 9, 10; spec 08 sections 4, 6) so a partial document is
//! valid. Hot reload: the stage reads a `watch::Receiver<TtsConfig>` per sentence, and the slot
//! gate is resized live. Not hot: the output sample rate and warm pool size (those live in
//! `warptalk_cartesia::CartesiaConfig`: `sample_rate` 24000 or 44100, `pool_size`).

use std::time::Duration;

use serde::Deserialize;

use crate::error::TtsError;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    /// Spec: 3600 s (about 2 ms on a hit against about 1 s to synthesize).
    pub ttl_s: u64,
    /// Replay granularity of a cached sentence, so a hit has the same chunk shape as a stream.
    pub replay_chunk_ms: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_s: 3600,
            replay_chunk_ms: 100,
        }
    }
}

/// Delivery speed bounds. The vendor accepts 0.6 to 1.5 (spec 13 section 9).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct SpeedConfig {
    /// Baseline multiplier before isochrony and catch-up; 1.0 means "say nothing" to the vendor.
    pub base: f32,
    pub min: f32,
    pub max: f32,
}

impl Default for SpeedConfig {
    fn default() -> Self {
        Self {
            base: 1.0,
            min: 0.6,
            max: 1.5,
        }
    }
}

/// Closed-loop isochrony (spec 08 item 17): the speed is centred on each speaker's measured
/// dub-to-source duration ratio so a dub fits the speaker's gap and does not accumulate backlog.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct IsochronyConfig {
    pub enabled: bool,
    pub ema_alpha: f32,
    pub min_samples: u32,
    pub min_center: f32,
    pub max_center: f32,
    /// Pairs shorter than this are too short to time reliably.
    pub min_measurable_ms: u32,
    /// Ratios outside this band are measurement faults, not slow dubs.
    pub min_plausible_ratio: f32,
    pub max_plausible_ratio: f32,
}

impl Default for IsochronyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ema_alpha: 0.25,
            min_samples: 2,
            min_center: 0.8,
            max_center: 1.25,
            min_measurable_ms: 400,
            min_plausible_ratio: 0.25,
            max_plausible_ratio: 4.0,
        }
    }
}

/// Catch-up speech (spec 08 item 16): a dub behind the speaker reads faster, ramping linearly
/// from `on_time_ms` of lag (no change) to `full_lag_ms` (full boost). Nothing is dropped.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct CatchUpConfig {
    pub enabled: bool,
    pub on_time_ms: u64,
    pub full_lag_ms: u64,
    /// Deliberately under the vendor maximum: it multiplies the speaker's own rate.
    pub max_speed: f32,
}

impl Default for CatchUpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_time_ms: 1500,
            full_lag_ms: 4000,
            max_speed: 1.25,
        }
    }
}

/// Bounded retry, only for transient errors (spec 13 section 10).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub backoff_ms: u64,
    /// No retry starts after this long since the first attempt.
    pub window_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 1,
            backoff_ms: 500,
            window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Keep a vendor context open for the next sentence when one is already queued (prosody
    /// continuity). Off: every sentence is its own context.
    pub continuity: bool,
    /// A context idle longer than this is not trusted (the vendor retires idle ones on its own;
    /// upstream: 4 s, measured retirement 4.7-5.2 s).
    pub idle_expiry_ms: u64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            continuity: true,
            idle_expiry_ms: 4000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct QualityConfig {
    pub min_rms: f32,
    pub min_active_speech_ratio: f32,
    pub max_clipped_ratio: f32,
    pub min_active_energy_variation: f32,
}

impl Default for QualityConfig {
    fn default() -> Self {
        // Spec 13 section 6.1.
        Self {
            min_rms: 0.008,
            min_active_speech_ratio: 0.18,
            max_clipped_ratio: 0.02,
            min_active_energy_variation: 0.08,
        }
    }
}

/// Progressive in-meeting clone ladder (spec 13 section 6.1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct CloneConfig {
    /// Platform flag `flags.voice_clone`.
    pub enabled: bool,
    /// Clone at these accumulated speech durations (seconds): 3, 8, 45.
    pub ladder_seconds: Vec<f32>,
    /// The first dub waits at most this long from clone start, then uses the stock voice.
    pub first_wait_ms: u64,
    pub max_upgrades: u32,
    /// A later rung replaces the current clone only if its score beats it by this much.
    pub upgrade_margin: f32,
    pub max_buffer_seconds: f32,
    /// Text shorter than this uses the stock voice.
    pub min_clone_chars: usize,
    pub quality: QualityConfig,
}

impl Default for CloneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ladder_seconds: vec![3.0, 8.0, 45.0],
            first_wait_ms: 2500,
            max_upgrades: 1,
            upgrade_margin: 0.15,
            max_buffer_seconds: 90.0,
            min_clone_chars: 8,
            quality: QualityConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Concurrent vendor generations, counting open contexts (spec: 3; the plan limit divided by
    /// replicas). The vendor queues the excess silently, so this gate is the only visible queue.
    pub max_concurrency: usize,
    pub cache: CacheConfig,
    pub speed: SpeedConfig,
    pub isochrony: IsochronyConfig,
    pub catch_up: CatchUpConfig,
    pub retry: RetryConfig,
    pub context: ContextConfig,
    pub clone: CloneConfig,
    /// Per (room, speaker, language) inbox; sentences of one key are spoken strictly in order.
    pub key_inbox_capacity: usize,
    /// A key worker with nothing to do for this long exits (and closes its context).
    pub key_idle_ms: u64,
    /// How long a spoken (segment, sentence, language) is remembered to drop duplicate requests
    /// from several listeners of the same language (ADR D5: synthesize once, fan out).
    pub dedupe_ttl_ms: u64,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 3,
            cache: CacheConfig::default(),
            speed: SpeedConfig::default(),
            isochrony: IsochronyConfig::default(),
            catch_up: CatchUpConfig::default(),
            retry: RetryConfig::default(),
            context: ContextConfig::default(),
            clone: CloneConfig::default(),
            key_inbox_capacity: 64,
            key_idle_ms: 60_000,
            dedupe_ttl_ms: 60_000,
        }
    }
}

impl TtsConfig {
    pub fn validate(&self) -> Result<(), TtsError> {
        let bad = |m: &str| Err(TtsError::InvalidConfig(m.into()));
        if self.max_concurrency == 0 {
            return bad("max_concurrency must be > 0");
        }
        if self.key_inbox_capacity == 0 {
            return bad("key_inbox_capacity must be > 0");
        }
        if !(0.0 < self.speed.min && self.speed.min <= self.speed.max) {
            return bad("speed.min/max invalid");
        }
        if !(self.speed.min..=self.speed.max).contains(&self.speed.base) {
            return bad("speed.base outside [min, max]");
        }
        let i = &self.isochrony;
        if !(0.0 < i.ema_alpha && i.ema_alpha <= 1.0)
            || i.min_center > i.max_center
            || i.min_plausible_ratio >= i.max_plausible_ratio
        {
            return bad("isochrony settings invalid");
        }
        let c = &self.catch_up;
        if c.full_lag_ms <= c.on_time_ms || c.max_speed < 1.0 || c.max_speed > self.speed.max {
            return bad(
                "catch_up: need on_time_ms < full_lag_ms and 1.0 <= max_speed <= speed.max",
            );
        }
        if self.cache.replay_chunk_ms == 0 {
            return bad("cache.replay_chunk_ms must be > 0");
        }
        let l = &self.clone.ladder_seconds;
        if l.is_empty() || l.windows(2).any(|w| w[0] >= w[1]) || l.iter().any(|s| *s <= 0.0) {
            return bad("clone.ladder_seconds must be positive and strictly increasing");
        }
        if self.clone.max_buffer_seconds < *l.last().unwrap_or(&0.0) {
            return bad("clone.max_buffer_seconds must cover the last ladder rung");
        }
        Ok(())
    }

    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache.ttl_s)
    }
    pub fn retry_backoff(&self) -> Duration {
        Duration::from_millis(self.retry.backoff_ms)
    }
    pub fn context_idle_expiry(&self) -> Duration {
        Duration::from_millis(self.context.idle_expiry_ms)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_and_validate() {
        let c = TtsConfig::default();
        c.validate().unwrap();
        assert_eq!(c.max_concurrency, 3);
        assert_eq!(c.cache.ttl_s, 3600);
        assert_eq!(c.catch_up.on_time_ms, 1500);
        assert_eq!(c.catch_up.full_lag_ms, 4000);
        assert!((c.catch_up.max_speed - 1.25).abs() < f32::EPSILON);
        assert!((c.isochrony.min_center - 0.8).abs() < f32::EPSILON);
        assert_eq!(c.retry.max_retries, 1);
        assert_eq!(c.retry.window_ms, 10_000);
        assert_eq!(c.clone.ladder_seconds, [3.0, 8.0, 45.0]);
        assert_eq!(c.clone.first_wait_ms, 2500);
    }

    #[test]
    fn partial_json_and_rejections() {
        let c: TtsConfig =
            serde_json::from_str(r#"{"max_concurrency":5,"catch_up":{"max_speed":1.2}}"#).unwrap();
        assert_eq!(c.max_concurrency, 5);
        assert!((c.catch_up.max_speed - 1.2).abs() < f32::EPSILON);
        assert_eq!(c.catch_up.on_time_ms, 1500);
        c.validate().unwrap();
        for bad in [
            r#"{"max_concurrency":0}"#,
            r#"{"catch_up":{"max_speed":1.9}}"#,
            r#"{"catch_up":{"on_time_ms":5000}}"#,
            r#"{"clone":{"ladder_seconds":[8,3]}}"#,
            r#"{"speed":{"base":3.0}}"#,
        ] {
            let c: TtsConfig = serde_json::from_str(bad).unwrap();
            assert!(c.validate().is_err(), "{bad}");
        }
    }
}
