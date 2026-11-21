//! PCM frame types, resampling, ring buffer, RMS/energy helpers. Pure and synchronous: no IO,
//! no async runtime. All audio is mono `f32` in `[-1.0, 1.0]`.
//!
//! WAV/Opus helpers are intentionally not here yet.

use std::sync::Arc;

/// 16 kHz: the rate STT and VAD consume.
pub const SAMPLE_RATE_16K: u32 = 16_000;
/// 24 kHz: a common TTS output rate.
pub const SAMPLE_RATE_24K: u32 = 24_000;
/// 48 kHz: WebRTC / LiveKit native rate.
pub const SAMPLE_RATE_48K: u32 = 48_000;

/// A cheaply clonable block of mono `f32` samples (clone = refcount bump, for fan-out).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub samples: Arc<[f32]>,
    pub sample_rate_hz: u32,
}

impl Frame {
    pub fn new(samples: Vec<f32>, sample_rate_hz: u32) -> Self {
        Self {
            samples: samples.into(),
            sample_rate_hz,
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn duration_ms(&self) -> u32 {
        frame_ms(self.samples.len(), self.sample_rate_hz)
    }

    pub fn rms(&self) -> f32 {
        rms(&self.samples)
    }
}

/// Duration in whole milliseconds of `len` samples at `hz` (0 when `hz == 0`).
pub fn frame_ms(len: usize, hz: u32) -> u32 {
    if hz == 0 {
        return 0;
    }
    u32::try_from(len as u64 * 1000 / u64::from(hz)).unwrap_or(u32::MAX)
}

/// Root-mean-square level. Empty input yields 0.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (sum_squares(samples) / samples.len() as f64).sqrt() as f32
}

/// Sum of squares accumulated in f64 (so long chunks do not lose precision).
pub fn sum_squares(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|&s| {
            let s = f64::from(s);
            s * s
        })
        .sum()
}

/// Peak absolute amplitude.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()))
}

/// `i16` PCM to `f32` in `[-1.0, 1.0)`.
pub fn i16_to_f32(input: &[i16]) -> Vec<f32> {
    input.iter().map(|&s| f32::from(s) / 32768.0).collect()
}

/// `f32` to `i16` PCM, clamped and rounded.
pub fn f32_to_i16(input: &[f32]) -> Vec<i16> {
    input
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

/// Simple resampler, good enough for 48k -> 16k/24k speech paths.
///
/// Downsampling uses area averaging (each output sample is the mean of the input span it
/// covers, a box low-pass that limits aliasing); upsampling uses linear interpolation.
/// TODO: switch to `rubato` (sinc) when the dependency is added to the workspace.
pub fn resample(input: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if input.is_empty() || from_hz == 0 || to_hz == 0 {
        return Vec::new();
    }
    if from_hz == to_hz {
        return input.to_vec();
    }
    let out_len = (input.len() as u64 * u64::from(to_hz) / u64::from(from_hz)) as usize;
    let ratio = f64::from(from_hz) / f64::from(to_hz); // input samples per output sample
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    if ratio > 1.0 {
        for i in 0..out_len {
            let start = i as f64 * ratio;
            let end = (start + ratio).min(input.len() as f64);
            let mut acc = 0.0_f64;
            let mut j = start.floor() as usize;
            while (j as f64) < end && j <= last {
                let lo = (j as f64).max(start);
                let hi = ((j + 1) as f64).min(end);
                acc += f64::from(input[j]) * (hi - lo);
                j += 1;
            }
            let span = end - start;
            out.push(if span > 0.0 { (acc / span) as f32 } else { 0.0 });
        }
    } else {
        for i in 0..out_len {
            let pos = i as f64 * ratio;
            let i0 = (pos.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = (pos - i0 as f64) as f32;
            out.push(input[i0] + (input[i1] - input[i0]) * frac);
        }
    }
    out
}

/// Fixed-capacity (in samples) ring: oldest samples are overwritten.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    buf: Vec<f32>,
    /// Next write position.
    head: usize,
    len: usize,
}

impl RingBuffer {
    pub fn new(capacity_samples: usize) -> Self {
        Self {
            buf: vec![0.0; capacity_samples],
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    pub fn push(&mut self, samples: &[f32]) {
        let cap = self.buf.len();
        if cap == 0 {
            return;
        }
        // Only the last `cap` samples can survive.
        let samples = &samples[samples.len().saturating_sub(cap)..];
        let first = samples.len().min(cap - self.head);
        self.buf[self.head..self.head + first].copy_from_slice(&samples[..first]);
        let rest = samples.len() - first;
        self.buf[..rest].copy_from_slice(&samples[first..]);
        self.head = (self.head + samples.len()) % cap;
        self.len = (self.len + samples.len()).min(cap);
    }

    /// The most recent `n` samples (fewer if less is stored), oldest first.
    pub fn take_last(&self, n: usize) -> Vec<f32> {
        let n = n.min(self.len);
        let cap = self.buf.len();
        let mut out = Vec::with_capacity(n);
        if n == 0 {
            return out;
        }
        let start = (self.head + cap - n) % cap;
        if start + n <= cap {
            out.extend_from_slice(&self.buf[start..start + n]);
        } else {
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..n - (cap - start)]);
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn rms_of_sine_and_empty() {
        assert_eq!(rms(&[]), 0.0);
        let r = rms(&sine(440.0, 16_000, 16_000, 0.5));
        assert!((r - 0.5 / 2f32.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn frame_ms_math() {
        assert_eq!(frame_ms(512, 16_000), 32);
        assert_eq!(frame_ms(960, 48_000), 20);
        assert_eq!(frame_ms(10, 0), 0);
    }

    #[test]
    fn i16_roundtrip() {
        let src = [0i16, 1, -1, 12345, -32768, 32767];
        let back = f32_to_i16(&i16_to_f32(&src));
        for (a, b) in src.iter().zip(&back) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1, "{a} vs {b}");
        }
        assert_eq!(f32_to_i16(&[2.0, -2.0]), vec![32767, -32767]);
    }

    #[test]
    fn ring_wraps_and_takes_last() {
        let mut r = RingBuffer::new(5);
        r.push(&[1.0, 2.0, 3.0]);
        assert_eq!(r.take_last(10), vec![1.0, 2.0, 3.0]);
        r.push(&[4.0, 5.0, 6.0, 7.0]);
        assert_eq!(r.len(), 5);
        assert_eq!(r.take_last(5), vec![3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(r.take_last(2), vec![6.0, 7.0]);
        r.push(&[10.0; 20]);
        assert_eq!(r.take_last(5), vec![10.0; 5]);
        r.clear();
        assert!(r.is_empty());
        assert!(RingBuffer::new(0).take_last(3).is_empty());
    }

    #[test]
    fn resample_lengths_and_level() {
        let src = sine(300.0, 48_000, 4800, 0.5);
        let d = resample(&src, 48_000, 16_000);
        assert_eq!(d.len(), 1600);
        assert!((rms(&d) - rms(&src)).abs() < 0.02);
        let u = resample(&d, 16_000, 24_000);
        assert_eq!(u.len(), 2400);
        assert!((rms(&u) - rms(&d)).abs() < 0.02);
        assert_eq!(resample(&src, 48_000, 48_000), src);
        assert!(resample(&[], 48_000, 16_000).is_empty());
    }

    #[test]
    fn frame_accessors() {
        let f = Frame::new(vec![0.5; 512], SAMPLE_RATE_16K);
        assert_eq!(f.duration_ms(), 32);
        assert!((f.rms() - 0.5).abs() < 1e-6);
        let g = f.clone();
        assert!(Arc::ptr_eq(&f.samples, &g.samples));
    }
}
