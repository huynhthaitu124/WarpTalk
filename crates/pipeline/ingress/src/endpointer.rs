use crate::{EndpointerConfig, SpeakerLevelFloor};

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const FRAME_MS: u32 = 32;
pub const FRAME_SAMPLES: usize = 512;
const SAMPLES_PER_MS: u64 = 16;

/// Root-mean-square level (empty input is 0).
pub(crate) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let s = f64::from(s);
            s * s
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Closed by the hangover ladder.
    Silence,
    /// Hit the chunk cap while the speaker was still talking.
    Cap,
    /// The stream ended (`Endpointer::finish`).
    EndOfStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    TooShort,
    BelowEnergyFloor,
    BelowSpeakerFloor,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EndpointEvent {
    /// Stream time (ms) at the start of the frame that opened the turn.
    SpeechStart { t_ms: u64 },
    /// Ungated audio for flash-mode streaming: from speech start, the first one carrying the
    /// pre-speech padding.
    LiveFrame { samples: Vec<f32> },
    UtteranceEnd {
        audio: Vec<f32>,
        speech_ms: u32,
        total_ms: u32,
        reason: EndReason,
        /// Trailing silence beyond the base hangover that was cut from `audio`.
        trimmed_trailing_ms: u32,
        /// Stream time (ms) at which the end was decided (end of the deciding frame).
        decided_at_ms: u64,
        /// Stream time (ms) at the end of the last speech frame. `decided_at_ms - this` is the
        /// end-of-speech to decision delay (the hangover) for the runtime to measure.
        last_speech_end_ms: u64,
        /// RMS of the speech portion, as judged by the energy floor.
        speech_rms: f32,
        /// Accepted only thanks to the speaker-relative floor.
        rescued_by_speaker_floor: bool,
    },
    Dropped {
        reason: DropReason,
        speech_ms: u32,
        total_ms: u32,
        decided_at_ms: u64,
    },
}

/// Fixed-capacity sample ring for the pre-speech padding.
struct PreRing {
    buf: Vec<f32>,
    head: usize,
    len: usize,
}

impl PreRing {
    fn new(cap: usize) -> Self {
        Self {
            buf: vec![0.0; cap],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, s: &[f32]) {
        let cap = self.buf.len();
        if cap == 0 {
            return;
        }
        let s = &s[s.len().saturating_sub(cap)..];
        let first = s.len().min(cap - self.head);
        self.buf[self.head..self.head + first].copy_from_slice(&s[..first]);
        self.buf[..s.len() - first].copy_from_slice(&s[first..]);
        self.head = (self.head + s.len()) % cap;
        self.len = (self.len + s.len()).min(cap);
    }

    /// Append everything stored (oldest first) to `out`.
    fn drain_into(&mut self, out: &mut Vec<f32>) {
        let cap = self.buf.len();
        if self.len > 0 {
            let start = (self.head + cap - self.len) % cap;
            if start + self.len <= cap {
                out.extend_from_slice(&self.buf[start..start + self.len]);
            } else {
                out.extend_from_slice(&self.buf[start..]);
                out.extend_from_slice(&self.buf[..self.len - (cap - start)]);
            }
        }
        self.len = 0;
        self.head = 0;
    }
}

/// Endpointing state machine. Feed one 32 ms frame plus its VAD probability at a time.
///
/// Rules (ported from `livekit_ingress_worker/worker.py`, spec 08 section 2.1):
/// - hysteresis: entering needs `vad_threshold`, staying needs only the release threshold;
/// - hangover ladder on SPEECH so far: `>= seek_after` -> 250 ms (boundary seeking),
///   `< short_turn_speech` -> 864 ms, otherwise 576 ms;
/// - silence counted beyond the base (576 ms) hangover is a decision window and is trimmed;
/// - the cap counts the whole buffer (pre-speech padding included) and fires on a speech frame;
/// - after the cap the speaker is still speaking: a new chunk starts empty and its speech
///   counter restarts at zero;
/// - gates run on the closed chunk: min speech, absolute floor on `rms / sqrt(speech share)`,
///   then the per-speaker relative floor. Live frames are never gated.
pub struct Endpointer {
    cfg: EndpointerConfig,
    entry: f32,
    release: f32,
    pre_ring: PreRing,
    floor: SpeakerLevelFloor,
    min_speech_samples: usize,
    cap_samples: usize,
    seek_after_samples: usize,
    short_turn_samples: usize,
    seek_hang: usize,
    base_hang: usize,
    short_hang: usize,
    /// Hangover once a sentence-complete hint arrived (0 = hint path disabled).
    hint_hang: usize,

    /// Stream clock: samples pushed so far.
    now_samples: u64,
    speaking: bool,
    buffer: Vec<f32>,
    streamed: usize,
    speech_samples: usize,
    silence_samples: usize,
    last_speech_end_samples: u64,
    published_this_turn: bool,
    /// The STT reported a finished sentence and no speech frame came since.
    sentence_hint: bool,
}

impl Endpointer {
    /// Panics never; an invalid config is the caller's problem (call `validate` first), values
    /// are only used arithmetically.
    pub fn new(cfg: EndpointerConfig) -> Self {
        let ms = |v: u32| (u64::from(v) * SAMPLES_PER_MS) as usize;
        Self {
            entry: cfg.vad_threshold,
            release: cfg.effective_release_threshold(),
            pre_ring: PreRing::new(ms(cfg.vad_pre_speech_ms)),
            floor: SpeakerLevelFloor::new(
                cfg.energy_floor_rms,
                cfg.energy_relative_ratio,
                cfg.energy_baseline_min_chunks,
                cfg.energy_baseline_window,
            ),
            min_speech_samples: ms(cfg.vad_min_speech_ms),
            cap_samples: ms(cfg.chunk_duration_ms),
            seek_after_samples: ms(cfg.vad_seek_boundary_after_ms),
            short_turn_samples: ms(cfg.vad_short_turn_speech_ms),
            seek_hang: ms(cfg.vad_seek_hangover_ms),
            base_hang: ms(cfg.vad_silence_hangover_ms),
            short_hang: ms(cfg.vad_short_turn_hangover_ms),
            hint_hang: ms(cfg.vad_sentence_hint_hangover_ms),
            buffer: Vec::with_capacity(ms(cfg.chunk_duration_ms) + FRAME_SAMPLES),
            cfg,
            now_samples: 0,
            speaking: false,
            streamed: 0,
            speech_samples: 0,
            silence_samples: 0,
            last_speech_end_samples: 0,
            published_this_turn: false,
            sentence_hint: false,
        }
    }

    pub fn config(&self) -> &EndpointerConfig {
        &self.cfg
    }

    pub fn speaker_floor(&self) -> &SpeakerLevelFloor {
        &self.floor
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Stream time in ms (end of the last pushed frame).
    pub fn now_ms(&self) -> u64 {
        self.now_samples / SAMPLES_PER_MS
    }

    pub fn push(&mut self, frame: &[f32], prob: f32) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        let frame_start = self.now_samples;
        self.now_samples += frame.len() as u64;
        let active = if self.speaking {
            self.release
        } else {
            self.entry
        };

        if prob >= active {
            if !self.speaking {
                self.speaking = true;
                self.buffer.clear();
                self.pre_ring.drain_into(&mut self.buffer);
                self.streamed = 0;
                self.speech_samples = 0;
                self.published_this_turn = false;
                self.sentence_hint = false;
                events.push(EndpointEvent::SpeechStart {
                    t_ms: frame_start / SAMPLES_PER_MS,
                });
            }
            self.buffer.extend_from_slice(frame);
            self.speech_samples += frame.len();
            self.silence_samples = 0;
            // New speech: the finished sentence the hint spoke of is not the end of the turn.
            self.sentence_hint = false;
            self.last_speech_end_samples = self.now_samples;
            // Everything not yet streamed (pre-speech, any silence since the last speech
            // frame, this frame) goes out ungated.
            events.push(EndpointEvent::LiveFrame {
                samples: self.buffer[self.streamed..].to_vec(),
            });
            self.streamed = self.buffer.len();

            if self.buffer.len() >= self.cap_samples {
                self.finalize(EndReason::Cap, &mut events);
            }
        } else if self.speaking {
            self.buffer.extend_from_slice(frame);
            self.silence_samples += frame.len();
            let hangover = self.current_hangover();
            if self.silence_samples >= hangover {
                self.finalize(EndReason::Silence, &mut events);
            }
        } else {
            self.pre_ring.push(frame);
        }
        events
    }

    /// Hangover that applies right now: the ladder rung for the speech so far, shortened to the
    /// hint hangover while a sentence-complete hint is armed (never lengthened).
    fn current_hangover(&self) -> usize {
        let rung = if self.speech_samples >= self.seek_after_samples {
            self.seek_hang
        } else if self.speech_samples < self.short_turn_samples {
            self.short_hang
        } else {
            self.base_hang
        };
        if self.sentence_hint && self.hint_hang > 0 {
            rung.min(self.hint_hang)
        } else {
            rung
        }
    }

    /// Semantic endpointing hint: the streaming STT already produced a sentence terminator for
    /// the turn in progress. While the VAD is in its hangover (the speaker is silent) the
    /// hangover is cut to `vad_sentence_hint_hangover_ms`; if that much silence has already
    /// elapsed the turn closes right away and the events are returned. Pure and idempotent:
    /// ignored when disabled (0 ms), when no turn is open, while the speaker is talking, and
    /// disarmed by the next speech frame, so a hint about an earlier sentence cannot cut a
    /// turn that went on.
    pub fn hint_sentence_complete(&mut self) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        if self.hint_hang == 0 || !self.speaking || self.silence_samples == 0 {
            return events;
        }
        self.sentence_hint = true;
        if self.silence_samples >= self.current_hangover() {
            self.finalize(EndReason::Silence, &mut events);
        }
        events
    }

    /// End of stream: closes an open turn (subject to the same gates).
    pub fn finish(&mut self) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        if self.speaking {
            self.finalize(EndReason::EndOfStream, &mut events);
        }
        events
    }

    fn finalize(&mut self, reason: EndReason, events: &mut Vec<EndpointEvent>) {
        let decided_at_ms = self.now_ms();
        let last_speech_end_ms = self.last_speech_end_samples / SAMPLES_PER_MS;

        // Silence beyond the base hangover was waiting time, not padding: do not ship it.
        let excess = self.silence_samples.saturating_sub(self.base_hang);
        if excess > 0 {
            self.buffer.truncate(self.buffer.len() - excess);
        }
        let speech_samples = self.speech_samples;
        let total_samples = self.buffer.len();
        let speech_ms = (speech_samples as u64 / SAMPLES_PER_MS) as u32;
        let total_ms = (total_samples as u64 / SAMPLES_PER_MS) as u32;
        let trimmed_trailing_ms = (excess as u64 / SAMPLES_PER_MS) as u32;

        let mut accepted = false;
        if speech_samples < self.min_speech_samples && !self.published_this_turn {
            events.push(EndpointEvent::Dropped {
                reason: DropReason::TooShort,
                speech_ms,
                total_ms,
                decided_at_ms,
            });
        } else {
            let share = (speech_samples > 0 && speech_samples < total_samples)
                .then(|| speech_samples as f32 / total_samples as f32);
            let verdict = self.floor.judge(rms(&self.buffer), share);
            if verdict.accept {
                accepted = true;
                events.push(EndpointEvent::UtteranceEnd {
                    audio: self.buffer.clone(),
                    speech_ms,
                    total_ms,
                    reason,
                    trimmed_trailing_ms,
                    decided_at_ms,
                    last_speech_end_ms,
                    speech_rms: verdict.speech_rms,
                    rescued_by_speaker_floor: verdict.relative,
                });
            } else {
                self.floor.note_dropped(speech_ms);
                let reason = if verdict.relative_floor.is_some() {
                    DropReason::BelowSpeakerFloor
                } else {
                    DropReason::BelowEnergyFloor
                };
                events.push(EndpointEvent::Dropped {
                    reason,
                    speech_ms,
                    total_ms,
                    decided_at_ms,
                });
            }
        }

        self.buffer.clear();
        self.streamed = 0;
        self.speech_samples = 0;
        self.silence_samples = 0;
        self.sentence_hint = false;
        if reason == EndReason::Cap {
            // Still mid-utterance: the speaker keeps talking into a fresh chunk.
            self.published_this_turn |= accepted;
        } else {
            self.speaking = false;
            self.published_this_turn = false;
            // Whatever the ring held predates this turn.
            self.pre_ring.drain_into(&mut Vec::new());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{EnergyVad, Vad};

    /// Deterministic generator: bursts of sine, silence and low noise, in 32 ms frames.
    struct Gen {
        frames: Vec<Vec<f32>>,
        phase: f32,
        rng: u32,
    }

    impl Gen {
        fn new() -> Self {
            Self {
                frames: Vec::new(),
                phase: 0.0,
                rng: 0x1234_5678,
            }
        }
        fn noise(&mut self) -> f32 {
            self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((self.rng >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
        }
        /// `ms` of sine at `amp` (peak), plus noise at 0.001.
        fn tone(&mut self, ms: u32, amp: f32) -> &mut Self {
            for _ in 0..ms / FRAME_MS {
                let mut f = Vec::with_capacity(FRAME_SAMPLES);
                for _ in 0..FRAME_SAMPLES {
                    self.phase += 2.0 * std::f32::consts::PI * 220.0 / SAMPLE_RATE_HZ as f32;
                    let n = self.noise() * 0.001;
                    f.push(amp * self.phase.sin() + n);
                }
                self.frames.push(f);
            }
            self
        }
        fn silence(&mut self, ms: u32) -> &mut Self {
            for _ in 0..ms / FRAME_MS {
                let f: Vec<f32> = (0..FRAME_SAMPLES).map(|_| self.noise() * 0.001).collect();
                self.frames.push(f);
            }
            self
        }
    }

    /// Run frames through an EnergyVad-driven endpointer; collect all events.
    fn run(ep: &mut Endpointer, g: &Gen) -> Vec<EndpointEvent> {
        let mut vad = EnergyVad::default();
        let mut out = Vec::new();
        for f in &g.frames {
            let p = vad.speech_prob(f);
            out.extend(ep.push(f, p));
        }
        out
    }

    fn ends(ev: &[EndpointEvent]) -> Vec<&EndpointEvent> {
        ev.iter()
            .filter(|e| matches!(e, EndpointEvent::UtteranceEnd { .. }))
            .collect()
    }

    fn drops(ev: &[EndpointEvent]) -> Vec<DropReason> {
        ev.iter()
            .filter_map(|e| match e {
                EndpointEvent::Dropped { reason, .. } => Some(*reason),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn short_turn_uses_864_hangover_and_trims_to_576() {
        let mut g = Gen::new();
        g.silence(640).tone(704, 0.3).silence(1600);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        let e = ends(&ev);
        assert_eq!(e.len(), 1);
        let EndpointEvent::UtteranceEnd {
            reason,
            decided_at_ms,
            last_speech_end_ms,
            trimmed_trailing_ms,
            speech_ms,
            total_ms,
            audio,
            ..
        } = e[0]
        else {
            unreachable!()
        };
        assert_eq!(*reason, EndReason::Silence);
        assert_eq!(decided_at_ms - last_speech_end_ms, 864);
        assert_eq!(*trimmed_trailing_ms, 288);
        assert_eq!(*speech_ms, 704);
        // pre-speech 384 + speech 704 + base hangover 576
        assert_eq!(*total_ms, 384 + 704 + 576);
        assert_eq!(audio.len(), (*total_ms as usize) * 16);
    }

    #[test]
    fn mid_turn_uses_576_hangover() {
        let mut g = Gen::new();
        g.tone(2016, 0.3).silence(1600);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        let EndpointEvent::UtteranceEnd {
            decided_at_ms,
            last_speech_end_ms,
            trimmed_trailing_ms,
            ..
        } = ends(&ev)[0]
        else {
            unreachable!()
        };
        assert_eq!(decided_at_ms - last_speech_end_ms, 576);
        assert_eq!(*trimmed_trailing_ms, 0);
    }

    #[test]
    fn long_turn_cuts_at_250ms_pause() {
        let mut g = Gen::new();
        // 4.5 s speech, 256 ms pause, then more speech, then a long silence.
        g.tone(4512, 0.3).silence(256).tone(1024, 0.3).silence(1600);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        let e = ends(&ev);
        assert_eq!(e.len(), 2, "{ev:?}");
        let EndpointEvent::UtteranceEnd {
            reason,
            decided_at_ms,
            last_speech_end_ms,
            speech_ms,
            ..
        } = e[0]
        else {
            unreachable!()
        };
        assert_eq!(*reason, EndReason::Silence);
        assert_eq!(decided_at_ms - last_speech_end_ms, 256); // 8 frames, the 250 ms rung
        assert_eq!(*speech_ms, 4512);
    }

    #[test]
    fn cap_at_6000_counts_padding_and_continues() {
        let mut g = Gen::new();
        g.silence(640).tone(9000, 0.3).silence(1600);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        let e = ends(&ev);
        assert!(e.len() >= 2, "{ev:?}");
        let EndpointEvent::UtteranceEnd {
            reason, total_ms, ..
        } = e[0]
        else {
            unreachable!()
        };
        assert_eq!(*reason, EndReason::Cap);
        // 384 ms pre-speech counts: cap fires on the first frame reaching 6000 ms.
        assert!((6000..6000 + FRAME_MS).contains(total_ms), "{total_ms}");
        let EndpointEvent::UtteranceEnd { reason, .. } = e[e.len() - 1] else {
            unreachable!()
        };
        assert_eq!(*reason, EndReason::Silence);
    }

    #[test]
    fn pre_speech_padding_included_in_audio_and_live_frame() {
        let mut g = Gen::new();
        g.silence(1000).tone(640, 0.3).silence(1000);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        let first_live = ev
            .iter()
            .find_map(|e| match e {
                EndpointEvent::LiveFrame { samples } => Some(samples.len()),
                _ => None,
            })
            .unwrap();
        assert_eq!(first_live, 384 * 16 + FRAME_SAMPLES);
        let EndpointEvent::UtteranceEnd { total_ms, .. } = ends(&ev)[0] else {
            unreachable!()
        };
        assert_eq!(*total_ms, 384 + 640 + 576);
        // SpeechStart precedes the first LiveFrame.
        let i_start = ev
            .iter()
            .position(|e| matches!(e, EndpointEvent::SpeechStart { .. }))
            .unwrap();
        assert!(matches!(ev[i_start + 1], EndpointEvent::LiveFrame { .. }));
        assert_eq!(ev[i_start], EndpointEvent::SpeechStart { t_ms: 992 });
    }

    #[test]
    fn live_frames_cover_all_speech_ungated() {
        // Quiet (would be dropped by the energy floor) but live frames still flow.
        let mut g = Gen::new();
        g.tone(640, 0.02).silence(1000);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let mut out = Vec::new();
        for f in &g.frames {
            out.extend(ep.push(
                f,
                if f.iter().any(|s| s.abs() > 0.01) {
                    1.0
                } else {
                    0.0
                },
            ));
        }
        let live = out
            .iter()
            .filter(|e| matches!(e, EndpointEvent::LiveFrame { .. }))
            .count();
        assert_eq!(live, 20);
        assert_eq!(drops(&out), vec![DropReason::BelowEnergyFloor]);
    }

    #[test]
    fn too_short_dropped() {
        let mut g = Gen::new();
        g.silence(500).tone(160, 0.3).silence(1500);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let ev = run(&mut ep, &g);
        assert!(ends(&ev).is_empty());
        assert_eq!(drops(&ev), vec![DropReason::TooShort]);
        assert!(!ep.is_speaking());
    }

    #[test]
    fn low_energy_dropped() {
        let mut g = Gen::new();
        g.tone(800, 0.02).silence(1500); // rms ~0.014 < 0.02
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let mut out = Vec::new();
        for f in &g.frames {
            let p = if f.iter().any(|s| s.abs() > 0.01) {
                1.0
            } else {
                0.0
            };
            out.extend(ep.push(f, p));
        }
        assert_eq!(drops(&out), vec![DropReason::BelowEnergyFloor]);
        assert_eq!(ep.speaker_floor().dropped_chunks, 1);
    }

    fn run_prob(ep: &mut Endpointer, g: &Gen, thresh: f32) -> Vec<EndpointEvent> {
        let mut out = Vec::new();
        for f in &g.frames {
            let p = if f.iter().any(|s| s.abs() > thresh) {
                1.0
            } else {
                0.0
            };
            out.extend(ep.push(f, p));
        }
        out
    }

    #[test]
    fn quiet_speaker_rescued_by_speaker_floor() {
        let mut ep = Endpointer::new(EndpointerConfig::default());
        // Quiet utterance before any baseline exists is dropped.
        let mut g = Gen::new();
        g.tone(800, 0.025).silence(1500);
        let ev = run_prob(&mut ep, &g, 0.01);
        assert_eq!(drops(&ev), vec![DropReason::BelowEnergyFloor]);

        // Three loud utterances establish a baseline (~0.035 speech rms).
        let mut g = Gen::new();
        for _ in 0..3 {
            g.tone(800, 0.05).silence(1500);
        }
        let ev = run_prob(&mut ep, &g, 0.01);
        assert_eq!(ends(&ev).len(), 3);

        // Now the quiet one (0.025 peak -> ~0.0177 rms, >= 0.4 * 0.035) is rescued.
        let mut g = Gen::new();
        g.tone(800, 0.025).silence(1500);
        let ev = run_prob(&mut ep, &g, 0.01);
        let e = ends(&ev);
        assert_eq!(e.len(), 1, "{ev:?}");
        let EndpointEvent::UtteranceEnd {
            rescued_by_speaker_floor,
            ..
        } = e[0]
        else {
            unreachable!()
        };
        assert!(*rescued_by_speaker_floor);

        // Much quieter (noise-like) is still dropped, by the speaker floor.
        let mut g = Gen::new();
        g.tone(800, 0.01).silence(1500);
        let ev = run_prob(&mut ep, &g, 0.005);
        assert_eq!(drops(&ev), vec![DropReason::BelowSpeakerFloor]);
    }

    #[test]
    fn hysteresis_release_keeps_turn_open() {
        let cfg = EndpointerConfig::default();
        let mut ep = Endpointer::new(cfg);
        let f = vec![0.1_f32; FRAME_SAMPLES];
        let mut out = Vec::new();
        for _ in 0..20 {
            out.extend(ep.push(&f, 0.9));
        }
        // 0.25 is under entry (0.35) but over release (0.20): still speech.
        for _ in 0..60 {
            out.extend(ep.push(&f, 0.25));
        }
        assert!(ends(&out).is_empty());
        assert!(ep.is_speaking());
        // And 0.25 alone never starts a turn.
        let mut ep = Endpointer::new(EndpointerConfig::default());
        for _ in 0..20 {
            assert!(ep.push(&f, 0.25).is_empty());
        }
    }

    #[test]
    fn release_is_clamped_to_entry() {
        let cfg = EndpointerConfig {
            vad_threshold: 0.3,
            vad_release_threshold: 0.6,
            ..Default::default()
        };
        assert!((cfg.effective_release_threshold() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn finish_flushes_open_turn() {
        let mut g = Gen::new();
        g.tone(640, 0.3);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let mut ev = run(&mut ep, &g);
        ev.extend(ep.finish());
        let EndpointEvent::UtteranceEnd { reason, .. } = ends(&ev)[0] else {
            unreachable!()
        };
        assert_eq!(*reason, EndReason::EndOfStream);
    }

    #[test]
    fn config_partial_json_and_validation() {
        let cfg: EndpointerConfig =
            serde_json::from_str(r#"{"chunk_duration_ms": 4000, "vad_threshold": 0.5}"#).unwrap();
        assert_eq!(cfg.chunk_duration_ms, 4000);
        assert_eq!(cfg.vad_threshold, 0.5);
        assert_eq!(cfg.vad_pre_speech_ms, 384);
        assert_eq!(cfg.vad_short_turn_hangover_ms, 864);
        assert_eq!(cfg.energy_relative_ratio, 0.4);
        cfg.validate().unwrap();
        let empty: EndpointerConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, EndpointerConfig::default());
        empty.validate().unwrap();

        let bad = |f: fn(&mut EndpointerConfig)| {
            let mut c = EndpointerConfig::default();
            f(&mut c);
            assert!(c.validate().is_err());
        };
        bad(|c| c.chunk_duration_ms = 1999);
        bad(|c| c.chunk_duration_ms = 15001);
        bad(|c| c.vad_threshold = 0.0);
        bad(|c| c.vad_threshold = 1.5);
        bad(|c| c.vad_seek_hangover_ms = 700);
        bad(|c| c.vad_short_turn_speech_ms = 5000);
        bad(|c| c.energy_relative_ratio = 2.0);
        bad(|c| c.energy_baseline_min_chunks = 9);
    }

    fn hint_cfg(ms: u32) -> EndpointerConfig {
        EndpointerConfig {
            vad_sentence_hint_hangover_ms: ms,
            ..EndpointerConfig::default()
        }
    }

    /// Pushes frames one at a time, calling `hint` after `hint_after` silent frames.
    fn run_with_hint(
        ep: &mut Endpointer,
        g: &Gen,
        hint_after_silent_frames: Option<usize>,
    ) -> (Vec<EndpointEvent>, bool) {
        let mut vad = EnergyVad::default();
        let mut out = Vec::new();
        let mut silent = 0usize;
        let mut hinted_in_hangover = false;
        for f in &g.frames {
            let p = vad.speech_prob(f);
            let ev = ep.push(f, p);
            let ended = !ev.is_empty()
                && ev
                    .iter()
                    .any(|e| matches!(e, EndpointEvent::UtteranceEnd { .. }));
            out.extend(ev);
            if ep.is_speaking() && p < 0.2 {
                silent += 1;
                if Some(silent) == hint_after_silent_frames {
                    hinted_in_hangover = true;
                    out.extend(ep.hint_sentence_complete());
                }
            } else if !ended {
                silent = 0;
            }
        }
        (out, hinted_in_hangover)
    }

    fn hangover_of(ev: &[EndpointEvent]) -> u64 {
        let EndpointEvent::UtteranceEnd {
            decided_at_ms,
            last_speech_end_ms,
            ..
        } = ends(ev)[0]
        else {
            unreachable!()
        };
        decided_at_ms - last_speech_end_ms
    }

    #[test]
    fn hint_shortens_mid_turn_hangover_to_configured_value() {
        let mut g = Gen::new();
        g.tone(2016, 0.3).silence(1600);
        let mut ep = Endpointer::new(hint_cfg(250));
        // Hint arrives with the first silent frame: end at 250 ms rounded up to frames (256).
        let (ev, hinted) = run_with_hint(&mut ep, &g, Some(1));
        assert!(hinted);
        assert_eq!(ends(&ev).len(), 1);
        assert_eq!(hangover_of(&ev), 256);
    }

    #[test]
    fn hint_after_enough_silence_closes_the_turn_immediately() {
        let mut g = Gen::new();
        g.tone(2016, 0.3).silence(1600);
        let mut ep = Endpointer::new(hint_cfg(250));
        // 10 silent frames = 320 ms >= 250 ms: the hint itself returns the end.
        let (ev, _) = run_with_hint(&mut ep, &g, Some(10));
        assert_eq!(ends(&ev).len(), 1);
        assert_eq!(hangover_of(&ev), 320);
    }

    #[test]
    fn hint_disabled_by_default_and_without_flag_nothing_changes() {
        let mut g = Gen::new();
        g.tone(2016, 0.3).silence(1600);
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let (ev, _) = run_with_hint(&mut ep, &g, Some(1));
        assert_eq!(hangover_of(&ev), 576);
        let mut ep = Endpointer::new(hint_cfg(250));
        let (ev, _) = run_with_hint(&mut ep, &g, None);
        assert_eq!(hangover_of(&ev), 576);
    }

    #[test]
    fn hint_never_lengthens_the_hangover() {
        // Long turn: seek rung is 250 already; a 500 ms hint must not stretch it.
        let mut g = Gen::new();
        g.tone(4500, 0.3).silence(1600);
        let mut ep = Endpointer::new(hint_cfg(500));
        let (ev, _) = run_with_hint(&mut ep, &g, Some(1));
        assert_eq!(hangover_of(&ev), 256);
    }

    #[test]
    fn hint_is_ignored_while_speaking_or_idle_and_disarmed_by_speech() {
        let mut ep = Endpointer::new(hint_cfg(250));
        assert!(ep.hint_sentence_complete().is_empty()); // idle
        let mut g = Gen::new();
        g.tone(2016, 0.3);
        let mut vad = EnergyVad::default();
        for f in &g.frames {
            ep.push(f, vad.speech_prob(f));
        }
        // Speaking, no silence yet: ignored (must not arm).
        assert!(ep.hint_sentence_complete().is_empty());
        // A short pause (96 ms), hint armed, then speech resumes: disarmed, ladder applies.
        let mut g2 = Gen::new();
        g2.silence(96);
        for f in &g2.frames {
            ep.push(f, vad.speech_prob(f));
        }
        assert!(ep.hint_sentence_complete().is_empty());
        let mut g3 = Gen::new();
        g3.tone(640, 0.3).silence(1600);
        let mut ev = Vec::new();
        for f in &g3.frames {
            ev.extend(ep.push(f, vad.speech_prob(f)));
        }
        assert_eq!(ends(&ev).len(), 1);
        assert_eq!(
            hangover_of(&ev),
            576,
            "stale hint must not cut the continued turn"
        );
    }

    #[test]
    fn hint_config_validation() {
        let mut c = EndpointerConfig::default();
        c.vad_sentence_hint_hangover_ms = 577;
        assert!(c.validate().is_err());
        c.vad_sentence_hint_hangover_ms = 250;
        assert!(c.validate().is_ok());
    }

    /// Micro benchmark: `cargo test -p warptalk-ingress --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_push_ns_per_frame() {
        let mut g = Gen::new();
        for _ in 0..8 {
            g.tone(2000, 0.3).silence(1000);
        }
        let mut ep = Endpointer::new(EndpointerConfig::default());
        let probs: Vec<f32> = g
            .frames
            .iter()
            .map(|f| if rms(f) > 0.01 { 1.0 } else { 0.0 })
            .collect();
        let rounds = 200;
        let start = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..rounds {
            for (f, p) in g.frames.iter().zip(&probs) {
                n += std::hint::black_box(ep.push(std::hint::black_box(f), *p)).len();
            }
        }
        let el = start.elapsed();
        let frames = rounds * g.frames.len();
        println!(
            "Endpointer::push: {} ns/frame ({} frames, {} events)",
            el.as_nanos() / frames as u128,
            frames,
            n
        );
    }
}
