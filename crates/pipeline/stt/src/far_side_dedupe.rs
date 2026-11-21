//! Text-level far-side dedupe: drop a bridge stand-in segment that is a delayed copy of a real
//! participant's own recent segment.
//!
//! Port of upstream `warptalk-ai/stt_worker/far_side_dedupe.py` (forward direction only; the
//! reverse "leak" rule is not ported). A WarpTalk user who is also in the Meet speaks into their
//! own mic; Meet plays the sentence back and the bridge stand-in captures it again roughly
//! 150-2000 ms later. The stand-in copy is the duplicate and is dropped.
//!
//! Fail-open: whenever timing is unknown, the language differs, the text is too short, or the
//! similarity is below the threshold, the segment is kept.
//!
//! Similarity follows upstream: both texts are normalized (lowercased, punctuation and whitespace
//! collapsed to single spaces), then compared at character level with a port of Python's
//! `difflib.SequenceMatcher` (including its autojunk heuristic for long sequences). Character
//! level needs no word segmentation, so CJK text without spaces works as-is. Combining marks are
//! kept so Vietnamese tone and vowel marks count; NFC input is assumed, as upstream does.
//!
//! Timing: upstream compares audio start times. The participant side here only carries an end
//! time, so lag and window are measured on segment END times (`end_ms` of the stand-in minus
//! `end_ms` of the participant line). The stand-in `start_ms` is used only for validation.

use std::collections::VecDeque;

use serde::Deserialize;
use thiserror::Error;

/// Participant segments kept for matching. Eviction by window keeps the buffer small in practice;
/// this cap bounds it regardless of the rate of observations.
const MAX_OBSERVED_SEGMENTS: usize = 64;

/// Languages that cannot rule a match out (same set as upstream).
const UNKNOWN_LANGUAGES: &[&str] = &["", "auto", "unknown", "und"];

/// Configuration for [`FarSideDedupe`]. Defaults match upstream `STTSettings`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct FarSideDedupeConfig {
    /// Master switch. When false, nothing is dropped and nothing is remembered.
    pub enabled: bool,
    /// Coarse prefilter: participant and stand-in end times must be within this many ms.
    pub window_ms: u64,
    /// Minimum similarity (0.0..=1.0) for a fuzzy match.
    pub min_ratio: f32,
    /// Stand-in text shorter than this (in normalized characters) is never dropped.
    pub min_chars: usize,
    /// Require compatible primary language subtags.
    pub same_language: bool,
    /// The stand-in must end at least this much after the participant line.
    pub min_lag_ms: u64,
    /// ...and at most this much after it.
    pub max_lag_ms: u64,
}

impl Default for FarSideDedupeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_ms: 15_000,
            min_ratio: 0.8,
            min_chars: 8,
            same_language: true,
            min_lag_ms: 150,
            max_lag_ms: 2_000,
        }
    }
}

/// Why a [`FarSideDedupeConfig`] was rejected by [`FarSideDedupeConfig::validate`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FarSideDedupeConfigError {
    #[error("window_ms must be greater than 0")]
    ZeroWindow,
    #[error("min_ratio must be a finite number in 0.0..=1.0")]
    RatioOutOfRange,
    #[error("min_chars must be at least 1")]
    ZeroMinChars,
    #[error("min_lag_ms ({min}) must not exceed max_lag_ms ({max})")]
    LagInverted { min: u64, max: u64 },
}

impl FarSideDedupeConfig {
    /// Check the configuration. Call once after deserializing; [`FarSideDedupe::new`] does not
    /// validate on its own.
    pub fn validate(&self) -> Result<(), FarSideDedupeConfigError> {
        if self.window_ms == 0 {
            return Err(FarSideDedupeConfigError::ZeroWindow);
        }
        if !self.min_ratio.is_finite() || !(0.0..=1.0).contains(&self.min_ratio) {
            return Err(FarSideDedupeConfigError::RatioOutOfRange);
        }
        if self.min_chars == 0 {
            return Err(FarSideDedupeConfigError::ZeroMinChars);
        }
        if self.min_lag_ms > self.max_lag_ms {
            return Err(FarSideDedupeConfigError::LagInverted {
                min: self.min_lag_ms,
                max: self.max_lag_ms,
            });
        }
        Ok(())
    }
}

/// Why a segment was kept or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupeReason {
    /// The dedupe is disabled in configuration.
    Disabled,
    /// The stand-in's start time is unknown (0 or after its end). Upstream treats 0 the same way.
    TimingUnknown,
    /// The stand-in text is shorter than `min_chars`.
    TooShort,
    /// No participant line matched within the window, lag band and similarity threshold.
    NoMatch,
    /// A participant line matched; the stand-in copy is dropped.
    FarSideEcho,
}

/// Result of [`FarSideDedupe::should_drop_far_side`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DedupeVerdict {
    pub drop: bool,
    /// Best similarity found against any candidate participant line (0.0 when none was compared).
    /// 1.0 when the stand-in is equal to or a contiguous piece of the participant line.
    pub matched_ratio: f32,
    pub reason: DedupeReason,
}

impl DedupeVerdict {
    fn keep(reason: DedupeReason, matched_ratio: f32) -> Self {
        Self {
            drop: false,
            matched_ratio,
            reason,
        }
    }
}

#[derive(Debug, Clone)]
struct ParticipantSegment {
    /// Normalized text (see [`normalize`]).
    text: Vec<char>,
    /// Primary language subtag, lowercased.
    language: String,
    end_ms: u64,
}

/// Remembers recent participant lines and decides whether a stand-in segment echoes one.
#[derive(Debug, Clone)]
pub struct FarSideDedupe {
    config: FarSideDedupeConfig,
    participants: VecDeque<ParticipantSegment>,
    latest_ms: u64,
}

impl FarSideDedupe {
    pub fn new(config: FarSideDedupeConfig) -> Self {
        Self {
            config,
            participants: VecDeque::new(),
            latest_ms: 0,
        }
    }

    pub fn config(&self) -> &FarSideDedupeConfig {
        &self.config
    }

    /// Number of participant lines currently remembered.
    pub fn observed_len(&self) -> usize {
        self.participants.len()
    }

    /// Record a line spoken by a named participant (not the stand-in).
    pub fn observe_participant(&mut self, text: &str, language: &str, end_ms: u64) {
        if !self.config.enabled {
            return;
        }
        let normalized = normalize(text);
        if normalized.is_empty() {
            return;
        }
        self.latest_ms = self.latest_ms.max(end_ms);
        self.participants.push_back(ParticipantSegment {
            text: normalized,
            language: primary_language(language),
            end_ms,
        });
        let window = self.config.window_ms;
        while let Some(front) = self.participants.front() {
            let expired = front.end_ms.saturating_add(window) < self.latest_ms;
            if expired || self.participants.len() > MAX_OBSERVED_SEGMENTS {
                self.participants.pop_front();
            } else {
                break;
            }
        }
    }

    /// Decide whether a stand-in segment is a delayed copy of a remembered participant line.
    pub fn should_drop_far_side(
        &self,
        text: &str,
        language: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> DedupeVerdict {
        if !self.config.enabled {
            return DedupeVerdict::keep(DedupeReason::Disabled, 0.0);
        }
        if start_ms == 0 || start_ms > end_ms {
            return DedupeVerdict::keep(DedupeReason::TimingUnknown, 0.0);
        }
        let candidate = normalize(text);
        if candidate.len() < self.config.min_chars {
            return DedupeVerdict::keep(DedupeReason::TooShort, 0.0);
        }
        let language = primary_language(language);

        let mut best_ratio = 0.0_f32;
        let mut matched = false;
        for seg in &self.participants {
            if end_ms.abs_diff(seg.end_ms) > self.config.window_ms {
                continue;
            }
            let lag = end_ms as i128 - seg.end_ms as i128;
            if lag < self.config.min_lag_ms as i128 || lag > self.config.max_lag_ms as i128 {
                continue;
            }
            if self.config.same_language && !languages_compatible(&language, &seg.language) {
                continue;
            }
            let ratio = match_ratio(&candidate, &seg.text, self.config.min_chars);
            best_ratio = best_ratio.max(ratio);
            if ratio >= self.config.min_ratio {
                matched = true;
            }
        }

        if matched {
            DedupeVerdict {
                drop: true,
                matched_ratio: best_ratio,
                reason: DedupeReason::FarSideEcho,
            }
        } else {
            DedupeVerdict::keep(DedupeReason::NoMatch, best_ratio)
        }
    }
}

/// Lowercase, keep letters, digits and combining marks, turn everything else into a single
/// space, and trim. Unicode-aware; no allocation beyond the returned vector.
fn normalize(text: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_alphanumeric() || is_combining_mark(c) {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// Common combining diacritical blocks. `char::is_alphanumeric` is false for these, but they
/// belong to the word (Vietnamese tone and vowel marks in decomposed form).
fn is_combining_mark(c: char) -> bool {
    matches!(
        c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

fn primary_language(language: &str) -> String {
    language
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .split('-')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn languages_compatible(a: &str, b: &str) -> bool {
    UNKNOWN_LANGUAGES.contains(&a) || UNKNOWN_LANGUAGES.contains(&b) || a == b
}

/// Similarity of a candidate against a participant line, following upstream `_text_match`:
/// containment of the candidate in the line counts as 1.0 (a piece of the line), a candidate that
/// contains the whole line plus more is not a copy (its extra words belong to someone else), and
/// otherwise the SequenceMatcher ratio is used. Lines shorter than `min_chars` never match.
fn match_ratio(candidate: &[char], line: &[char], min_chars: usize) -> f32 {
    if candidate.is_empty() || line.len() < min_chars {
        return 0.0;
    }
    if candidate == line || contains_run(line, candidate) {
        return 1.0;
    }
    sequence_ratio(candidate, line)
}

fn contains_run(haystack: &[char], needle: &[char]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Port of `difflib.SequenceMatcher(None, a, b).ratio()` with autojunk (sequences of 200 or more
/// items drop items that occur more than 1% of the time in `b`). `b` is the participant line.
fn sequence_ratio(a: &[char], b: &[char]) -> f32 {
    let popular = popular_items(b);
    let total = a.len() + b.len();
    let mut matches = 0_usize;
    let mut stack = vec![(0, a.len(), 0, b.len())];
    while let Some((alo, ahi, blo, bhi)) = stack.pop() {
        let (i, j, k) = find_longest_match(a, b, alo, ahi, blo, bhi, &popular);
        if k == 0 {
            continue;
        }
        matches += k;
        stack.push((alo, i, blo, j));
        stack.push((i + k, ahi, j + k, bhi));
    }
    (2.0 * matches as f64 / total as f64) as f32
}

/// Items of `b` that autojunk treats as popular (only for sequences of 200 or more).
fn popular_items(b: &[char]) -> Vec<char> {
    if b.len() < 200 {
        return Vec::new();
    }
    let limit = b.len() / 100 + 1;
    let mut sorted = b.to_vec();
    sorted.sort_unstable();
    let mut popular = Vec::new();
    let mut idx = 0;
    while idx < sorted.len() {
        let c = sorted[idx];
        let run = sorted[idx..].iter().take_while(|&&x| x == c).count();
        if run > limit {
            popular.push(c);
        }
        idx += run;
    }
    popular
}

/// Longest matching block in `a[alo..ahi]` / `b[blo..bhi]`, as `(i, j, k)`. Ties resolve to the
/// earliest `i`, then earliest `j`, as in difflib. Popular items never start a match but are
/// absorbed by the extension passes, which is what difflib does too.
fn find_longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
    popular: &[char],
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0_usize);
    // prev[j + 1] / cur[j + 1]: length of the match ending at a[i - 1] / a[i] and b[j].
    let mut prev = vec![0_usize; b.len() + 1];
    let mut cur = vec![0_usize; b.len() + 1];
    for (i, &ai) in a.iter().enumerate().take(ahi).skip(alo) {
        for (j, &bj) in b.iter().enumerate().take(bhi).skip(blo) {
            cur[j + 1] = 0;
            if ai != bj || popular.contains(&bj) {
                continue;
            }
            let k = prev[j] + 1;
            cur[j + 1] = k;
            if k > bestsize {
                besti = i + 1 - k;
                bestj = j + 1 - k;
                bestsize = k;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        besti -= 1;
        bestj -= 1;
        bestsize += 1;
    }
    while besti + bestsize < ahi
        && bestj + bestsize < bhi
        && a[besti + bestsize] == b[bestj + bestsize]
    {
        bestsize += 1;
    }
    (besti, bestj, bestsize)
}
