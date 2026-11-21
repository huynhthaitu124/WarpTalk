/// Voice activity detector scoring one 32 ms frame at a time.
pub trait Vad {
    /// Probability in `[0, 1]` that `frame_32ms` (512 samples at 16 kHz) contains speech.
    fn speech_prob(&mut self, frame_32ms: &[f32]) -> f32;
}

/// RMS-based stand-in for tests and as a fallback when no model is available.
/// Maps RMS linearly to `[0, 1]`, saturating at `full_scale_rms`.
#[derive(Debug, Clone)]
pub struct EnergyVad {
    pub full_scale_rms: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self {
            full_scale_rms: 0.05,
        }
    }
}

impl Vad for EnergyVad {
    fn speech_prob(&mut self, frame: &[f32]) -> f32 {
        if frame.is_empty() || self.full_scale_rms <= 0.0 {
            return 0.0;
        }
        let r = crate::endpointer::rms(frame);
        (r / self.full_scale_rms).clamp(0.0, 1.0)
    }
}
