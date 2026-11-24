//! P0 echo guard: detects when the microphone is mostly hearing the meeting audio played
//! through the speakers (no headphones).
//!
//! Method: every pushed chunk is summarised into 32 ms RMS slices per channel (a
//! [`LevelHistory`], filled synchronously by `push_audio`, indexed by position on the push
//! timeline). When a `me` utterance ends, [`evaluate`] looks at its slices: a slice is
//! *active* if `me` RMS exceeds `active_rms_floor`; it is *dominated* if `others` is also
//! active and louder than `me` by more than `dominance_db`. If dominated/active >=
//! `min_overlap_ratio` the utterance is marked `likely_echo`.
//!
//! Limits (by design, full AEC is P2): needs both channels pushed roughly in sync (the
//! `others` slices must already exist when the `me` utterance ends: the endpointer hangover
//! gives >= 576 ms of slack); a quiet `others` leaking into a loud `me` is not detected; real
//! double talk with `others` much louder than the user is a false positive. Action is
//! configurable: mark (default) or do not post.

use std::collections::VecDeque;

use crate::config::EchoGuardConfig;

pub const SLICE_MS: u64 = 32;
const MAX_SLICES: usize = 4096; // ~131 s

#[derive(Debug, Default)]
pub struct LevelHistory {
    ring: VecDeque<f32>,
    /// Slice index of `ring[0]`.
    first_idx: u64,
    acc_sumsq: f64,
    acc_n: usize,
    /// Position on the push timeline (ms, fractional).
    elapsed_ms: f64,
}

impl LevelHistory {
    /// Timeline position of the next sample, in whole ms.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms as u64
    }

    /// Account a chunk: advances the timeline and closes complete 32 ms slices.
    pub fn feed(&mut self, samples: &[f32], sample_rate: u32) {
        if sample_rate == 0 {
            return;
        }
        let slice_len = ((u64::from(sample_rate) * SLICE_MS) / 1000).max(1) as usize;
        for &s in samples {
            let s = f64::from(s);
            self.acc_sumsq += s * s;
            self.acc_n += 1;
            if self.acc_n >= slice_len {
                let rms = (self.acc_sumsq / self.acc_n as f64).sqrt() as f32;
                self.ring.push_back(rms);
                if self.ring.len() > MAX_SLICES {
                    self.ring.pop_front();
                    self.first_idx += 1;
                }
                self.acc_sumsq = 0.0;
                self.acc_n = 0;
            }
        }
        self.elapsed_ms += samples.len() as f64 * 1000.0 / f64::from(sample_rate);
    }

    pub fn rms_at(&self, slice_idx: u64) -> Option<f32> {
        let off = slice_idx.checked_sub(self.first_idx)?;
        self.ring.get(usize::try_from(off).ok()?).copied()
    }
}

/// `true` when the `me` utterance spanning `[start_ms, end_ms]` (push timeline) looks like
/// speaker bleed.
pub fn evaluate(
    me: &LevelHistory,
    others: &LevelHistory,
    start_ms: u64,
    end_ms: u64,
    cfg: &EchoGuardConfig,
) -> bool {
    if !cfg.enabled {
        return false;
    }
    let (mut active, mut dominated) = (0u32, 0u32);
    for idx in (start_ms / SLICE_MS)..=(end_ms / SLICE_MS) {
        let (Some(m), Some(o)) = (me.rms_at(idx), others.rms_at(idx)) else {
            continue;
        };
        if m <= cfg.active_rms_floor {
            continue;
        }
        active += 1;
        if o > cfg.active_rms_floor && 20.0 * (o / m).log10() > cfg.dominance_db {
            dominated += 1;
        }
    }
    active > 0 && dominated as f32 / active as f32 >= cfg.min_overlap_ratio
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hist(level: f32, ms: u64) -> LevelHistory {
        let mut h = LevelHistory::default();
        h.feed(&vec![level; (ms * 16) as usize], 16_000);
        h
    }

    #[test]
    fn timeline_and_slices() {
        let h = hist(0.1, 640);
        assert_eq!(h.elapsed_ms(), 640);
        assert!((h.rms_at(0).unwrap() - 0.1).abs() < 1e-6);
        assert!(h.rms_at(19).is_some());
        assert!(h.rms_at(20).is_none());
    }

    #[test]
    fn dominated_by_others_is_echo() {
        let cfg = EchoGuardConfig::default();
        // others 20 dB above me.
        assert!(evaluate(&hist(0.02, 1000), &hist(0.2, 1000), 0, 900, &cfg));
        // similar level: genuine double talk.
        assert!(!evaluate(&hist(0.2, 1000), &hist(0.2, 1000), 0, 900, &cfg));
        // others silent.
        assert!(!evaluate(&hist(0.2, 1000), &hist(0.0, 1000), 0, 900, &cfg));
        // disabled.
        let off = EchoGuardConfig {
            enabled: false,
            ..cfg
        };
        assert!(!evaluate(&hist(0.02, 1000), &hist(0.2, 1000), 0, 900, &off));
    }

    #[test]
    fn threshold_is_configurable_and_missing_others_is_not_echo() {
        let cfg = EchoGuardConfig {
            dominance_db: 30.0,
            ..EchoGuardConfig::default()
        };
        assert!(!evaluate(&hist(0.02, 1000), &hist(0.2, 1000), 0, 900, &cfg));
        let empty = LevelHistory::default();
        assert!(!evaluate(
            &hist(0.02, 1000),
            &empty,
            0,
            900,
            &EchoGuardConfig::default()
        ));
    }
}
