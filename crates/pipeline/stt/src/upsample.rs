//! Streaming 16 kHz -> 24 kHz resampler (polyphase windowed sinc, ratio 3:2).
//!
//! The realtime model takes 24 kHz pcm16. Linear interpolation from 16 kHz leaves strong
//! spectral images between 8 and 12 kHz and restarts at every 32 ms frame, a buzz the model
//! hears as consonant noise (the fricatives and final stops that tell Vietnamese words and
//! English word endings apart). This filter keeps history across frames, so a speaker's audio
//! is one continuous signal, and low-passes at the 16 kHz Nyquist.

use std::collections::VecDeque;
use std::f64::consts::PI;

const UP: u64 = 3;
const DOWN: u64 = 2;
/// Taps per polyphase branch; total filter length is `UP * TAPS_PER_PHASE`.
const TAPS_PER_PHASE: usize = 16;
/// Cutoff as a fraction of the 16 kHz Nyquist (leaves a transition band below 8 kHz).
const CUTOFF: f64 = 0.92;

#[derive(Debug, Clone)]
pub struct Upsampler16To24 {
    taps: Vec<f64>,
    /// Most recent input samples, oldest first; `history[0]` has absolute index `first`.
    history: VecDeque<f32>,
    first: u64,
    /// Input samples seen so far.
    consumed: u64,
    /// Next output position on the 48 kHz intermediate grid.
    next_t: u64,
}

impl Default for Upsampler16To24 {
    fn default() -> Self {
        Self::new()
    }
}

impl Upsampler16To24 {
    pub fn new() -> Self {
        let n = UP as usize * TAPS_PER_PHASE;
        let center = (n as f64 - 1.0) / 2.0;
        // Normalized to the 48 kHz grid: the input Nyquist (8 kHz) is 1/6 cycle per sample.
        let fc = CUTOFF / (2.0 * UP as f64);
        let mut taps: Vec<f64> = (0..n)
            .map(|k| {
                let x = k as f64 - center;
                let sinc = if x.abs() < 1e-12 {
                    2.0 * fc
                } else {
                    (2.0 * PI * fc * x).sin() / (PI * x)
                };
                // Blackman window.
                let w = 0.42 - 0.5 * (2.0 * PI * k as f64 / (n as f64 - 1.0)).cos()
                    + 0.08 * (4.0 * PI * k as f64 / (n as f64 - 1.0)).cos();
                sinc * w
            })
            .collect();
        // Zero-stuffing by UP divides the gain by UP: restore unity passband gain.
        let sum: f64 = taps.iter().sum();
        if sum.abs() > f64::EPSILON {
            for t in &mut taps {
                *t *= UP as f64 / sum;
            }
        }
        Self {
            taps,
            history: VecDeque::with_capacity(TAPS_PER_PHASE + 1),
            first: 0,
            consumed: 0,
            next_t: 0,
        }
    }

    /// Forgets the signal (call between utterances that are not contiguous in time).
    pub fn reset(&mut self) {
        self.history.clear();
        self.first = 0;
        self.consumed = 0;
        self.next_t = 0;
    }

    /// Resamples the next block; output length tracks 3/2 of the input over time.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len() * 3 / 2 + 2);
        let n = self.taps.len() as u64;
        for &s in input {
            self.history.push_back(s);
            self.consumed += 1;
            while self.history.len() > TAPS_PER_PHASE + 1 {
                self.history.pop_front();
                self.first += 1;
            }
            // Output at grid position t needs input index floor(t / UP).
            while self.next_t / UP < self.consumed {
                let t = self.next_t;
                let j_max = t / UP;
                let j_min = (t + UP).saturating_sub(n) / UP;
                let mut acc = 0.0_f64;
                let mut j = j_min.max(self.first);
                while j <= j_max {
                    let k = (t - UP * j) as usize;
                    if k < self.taps.len()
                        && let Some(x) = self.history.get((j - self.first) as usize)
                    {
                        acc += self.taps[k] * f64::from(*x);
                    }
                    j += 1;
                }
                out.push(acc as f32);
                self.next_t += DOWN;
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / rate).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn length_is_three_halves_and_frame_splits_do_not_matter() {
        let input = tone(440.0, 16_000.0, 16_000);
        let whole = Upsampler16To24::new().process(&input);
        assert_eq!(whole.len(), 24_000);
        let mut u = Upsampler16To24::new();
        let mut split = Vec::new();
        for chunk in input.chunks(512) {
            split.extend(u.process(chunk));
        }
        assert_eq!(split.len(), whole.len());
        for (a, b) in whole.iter().zip(&split) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn passband_keeps_level_and_matches_the_true_24k_signal() {
        let input = tone(1_000.0, 16_000.0, 16_000);
        let out = Upsampler16To24::new().process(&input);
        let steady = &out[200..23_800];
        let level = rms(steady) / rms(&input[200..15_800]);
        assert!((level - 1.0).abs() < 0.02, "gain {level}");
        // Compare with the tone rendered directly at 24 kHz, allowing for the filter delay.
        let delay = (UP as usize * TAPS_PER_PHASE - 1) as f64 / 2.0 / 48_000.0;
        let ideal: Vec<f32> = (200..23_800)
            .map(|i| (2.0 * PI * 1_000.0 * (i as f64 / 24_000.0 - delay)).sin() as f32 * 0.5)
            .collect();
        let err: Vec<f32> = steady.iter().zip(&ideal).map(|(a, b)| a - b).collect();
        assert!(rms(&err) < 0.01, "error rms {}", rms(&err));
    }

    #[test]
    fn imaging_is_suppressed_compared_with_linear() {
        // A 6 kHz tone images at 10 kHz after upsampling; measure energy above 8.5 kHz.
        let input = tone(6_000.0, 16_000.0, 16_000);
        let ours = Upsampler16To24::new().process(&input);
        let linear = warptalk_audio::resample(&input, 16_000, 24_000);
        let image = |x: &[f32]| {
            // Single-bin DFT at the 10 kHz image.
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            for (i, v) in x.iter().enumerate().skip(100) {
                let a = 2.0 * PI * 10_000.0 * i as f64 / 24_000.0;
                re += f64::from(*v) * a.cos();
                im += f64::from(*v) * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(image(&ours) * 10.0 < image(&linear));
    }

    #[test]
    fn reset_restarts_the_grid() {
        let mut u = Upsampler16To24::new();
        u.process(&[0.1; 5]);
        u.reset();
        assert_eq!(u.process(&[0.0; 2]).len(), 3);
    }
}
