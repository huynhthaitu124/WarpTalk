use std::collections::VecDeque;

/// Outcome of judging one chunk against the (per-speaker) energy floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorVerdict {
    pub accept: bool,
    /// RMS of the SPEECH in the chunk (raw RMS undiluted by padding).
    pub speech_rms: f32,
    pub absolute_floor: f32,
    /// Median speech level of recent absolute-clearing chunks; `None` until enough exist.
    pub baseline: Option<f32>,
    /// Level a chunk under the absolute floor still needs; `None` without a baseline.
    pub relative_floor: Option<f32>,
    /// True when accepted only through the speaker-relative rule.
    pub relative: bool,
}

/// One track's energy floor: the absolute floor, relaxed (never raised) for a speaker proven
/// to be quiet. One instance per speaker track for its whole lifetime.
///
/// Port of `livekit_ingress_worker/speech_level_floor.py`:
/// - anything the absolute floor accepts is accepted;
/// - the baseline learns only from chunks that cleared the ABSOLUTE floor;
/// - no baseline (fewer than `min_baseline_chunks`), no relaxation.
#[derive(Debug, Clone)]
pub struct SpeakerLevelFloor {
    absolute_floor: f32,
    ratio: f32,
    min_baseline: usize,
    window: usize,
    levels: VecDeque<f32>,
    pub dropped_chunks: u64,
    pub dropped_speech_ms: u64,
}

impl SpeakerLevelFloor {
    pub fn new(
        absolute_floor: f32,
        relative_ratio: f32,
        min_baseline_chunks: usize,
        baseline_window: usize,
    ) -> Self {
        let window = baseline_window.max(1);
        Self {
            absolute_floor,
            ratio: relative_ratio.max(0.0),
            min_baseline: min_baseline_chunks.max(1),
            window,
            levels: VecDeque::with_capacity(window),
            dropped_chunks: 0,
            dropped_speech_ms: 0,
        }
    }

    pub fn baseline(&self) -> Option<f32> {
        if self.levels.len() < self.min_baseline {
            return None;
        }
        let mut v: Vec<f32> = self.levels.iter().copied().collect();
        v.sort_by(f32::total_cmp);
        let n = v.len();
        Some(if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        })
    }

    /// `speech_share` is speech samples / total samples; `None` (or outside `(0, 1)`) means 1.
    /// The absolute floor is applied to `raw_rms / sqrt(share)`.
    pub fn judge(&mut self, raw_rms: f32, speech_share: Option<f32>) -> FloorVerdict {
        let share = speech_share.filter(|s| *s > 0.0 && *s < 1.0).unwrap_or(1.0);
        let speech_rms = raw_rms / share.sqrt();
        let baseline = self.baseline();
        let relative_floor = match baseline {
            Some(b) if self.ratio > 0.0 => Some(b * self.ratio),
            _ => None,
        };
        if speech_rms >= self.absolute_floor {
            if self.levels.len() == self.window {
                self.levels.pop_front();
            }
            self.levels.push_back(speech_rms);
            return FloorVerdict {
                accept: true,
                speech_rms,
                absolute_floor: self.absolute_floor,
                baseline,
                relative_floor,
                relative: false,
            };
        }
        let relative = relative_floor.is_some_and(|f| speech_rms >= f);
        FloorVerdict {
            accept: relative,
            speech_rms,
            absolute_floor: self.absolute_floor,
            baseline,
            relative_floor,
            relative,
        }
    }

    pub fn note_dropped(&mut self, speech_ms: u32) {
        self.dropped_chunks += 1;
        self.dropped_speech_ms += u64::from(speech_ms);
    }
}
