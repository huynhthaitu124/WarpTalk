//! Delivery speed: isochrony centring and catch-up, both from config.

use crate::config::{CatchUpConfig, IsochronyConfig, SpeedConfig, TtsConfig};
use crate::ports::GenConfig;

/// How a speaker's dubs have been running against the clock (dub / source duration, rolling).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DubFit {
    pub ratio: f32,
    pub samples: u32,
}

impl DubFit {
    pub const NONE: Self = Self {
        ratio: 1.0,
        samples: 0,
    };
}

/// Folds one completed dub in. Unusable pairs are skipped, not averaged (an empty dub would drag
/// the ratio to zero and make every later sentence slower).
pub fn observe(fit: DubFit, source_ms: u32, dub_ms: u32, c: &IsochronyConfig) -> DubFit {
    if source_ms < c.min_measurable_ms || dub_ms < c.min_measurable_ms {
        return fit;
    }
    let ratio = dub_ms as f32 / source_ms as f32;
    if !(c.min_plausible_ratio..=c.max_plausible_ratio).contains(&ratio) {
        return fit;
    }
    if fit.samples == 0 {
        return DubFit { ratio, samples: 1 };
    }
    DubFit {
        ratio: (1.0 - c.ema_alpha) * fit.ratio + c.ema_alpha * ratio,
        samples: fit.samples + 1,
    }
}

/// Tempo the next dub is centred on: exactly 1.0 until established. A dub running 20% long is
/// asked to speak 20% faster; clamped because the vendor damps `speed` and a naive controller
/// would wind up against the limit.
pub fn speed_center(fit: DubFit, c: &IsochronyConfig) -> f32 {
    if !c.enabled || fit.samples < c.min_samples || fit.ratio <= 0.0 {
        return 1.0;
    }
    fit.ratio.clamp(c.min_center, c.max_center)
}

/// Multiplier for a dub `lag_ms` behind the speaker: 1.0 up to `on_time_ms`, then linear to
/// `max_speed` at `full_lag_ms` (a step would be audible as the pace jumping).
pub fn catch_up_multiplier(lag_ms: u64, c: &CatchUpConfig) -> f32 {
    if !c.enabled || lag_ms <= c.on_time_ms || c.full_lag_ms <= c.on_time_ms {
        return 1.0;
    }
    let ramp = ((lag_ms - c.on_time_ms) as f32 / (c.full_lag_ms - c.on_time_ms) as f32).min(1.0);
    1.0 + ramp * (c.max_speed - 1.0)
}

/// Final vendor speed, clamped to the vendor range and rounded.
pub fn final_speed(center: f32, multiplier: f32, s: &SpeedConfig) -> f32 {
    let v = (s.base * center * multiplier).clamp(s.min, s.max);
    (v * 1000.0).round() / 1000.0
}

/// Delivery for one sentence; empty (say nothing) when the speed is the neutral 1.0.
pub fn gen_config(fit: DubFit, lag_ms: u64, cfg: &TtsConfig) -> GenConfig {
    let speed = final_speed(
        speed_center(fit, &cfg.isochrony),
        catch_up_multiplier(lag_ms, &cfg.catch_up),
        &cfg.speed,
    );
    GenConfig {
        speed: ((speed - 1.0).abs() > 1e-3).then_some(speed),
        ..GenConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_up_ramp() {
        let c = CatchUpConfig::default();
        assert!((catch_up_multiplier(0, &c) - 1.0).abs() < 1e-6);
        assert!((catch_up_multiplier(1500, &c) - 1.0).abs() < 1e-6);
        assert!((catch_up_multiplier(2750, &c) - 1.125).abs() < 1e-5);
        assert!((catch_up_multiplier(4000, &c) - 1.25).abs() < 1e-6);
        assert!((catch_up_multiplier(60_000, &c) - 1.25).abs() < 1e-6);
        let off = CatchUpConfig {
            enabled: false,
            ..c
        };
        assert!((catch_up_multiplier(4000, &off) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn isochrony_converges_clamps_and_skips_faults() {
        let c = IsochronyConfig::default();
        let mut f = DubFit::NONE;
        assert!((speed_center(f, &c) - 1.0).abs() < 1e-6);
        f = observe(f, 2000, 2600, &c); // 1.3
        assert!((speed_center(f, &c) - 1.0).abs() < 1e-6, "one sample is not a fit");
        f = observe(f, 2000, 2600, &c);
        assert!((speed_center(f, &c) - 1.25).abs() < 1e-6, "clamped to 1.25");
        let kept = observe(f, 100, 5000, &c);
        assert_eq!(kept, f, "too short: skipped");
        assert_eq!(observe(f, 1000, 9000, &c), f, "implausible: skipped");
        let slow = observe(DubFit::NONE, 2000, 1000, &c);
        let slow = observe(slow, 2000, 1000, &c);
        assert!((speed_center(slow, &c) - 0.8).abs() < 1e-6, "clamped to 0.8");
    }

    #[test]
    fn final_speed_clamped_and_neutral_says_nothing() {
        let cfg = TtsConfig::default();
        assert!(gen_config(DubFit::NONE, 0, &cfg).is_empty());
        let g = gen_config(DubFit::NONE, 4000, &cfg);
        assert_eq!(g.speed, Some(1.25));
        let fit = DubFit {
            ratio: 1.25,
            samples: 5,
        };
        // 1.25 centre * 1.25 catch-up = 1.5625, clamped to the vendor maximum.
        assert_eq!(gen_config(fit, 4000, &cfg).speed, Some(1.5));
    }
}
