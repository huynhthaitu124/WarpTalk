//! Rolling transcript context for the transcription `prompt`.
//!
//! The realtime transcription model hears each committed utterance on its own. Short, fast
//! utterances then lack the context that disambiguates them (Vietnamese homophones and tones,
//! names, an English term inside Vietnamese speech). Whisper-family models treat `prompt` as the
//! text that came right before the audio, so feeding back the speaker's own recent transcript
//! makes the next utterance continue the same words, names and spelling.
//!
//! Guards against the known failure of prompt feedback (the model echoing the prompt, or one
//! mistake reinforcing itself):
//! - only the last [`MAX_CONTEXT_CHARS`] characters are kept, cut at a word boundary;
//! - the context expires after [`CONTEXT_TTL`] without speech (a new topic starts clean);
//! - a segment that repeats text already in the tail is not appended again.

use std::time::{Duration, Instant};

/// Upper bound of the transcript tail sent as prompt (characters, not bytes).
pub const MAX_CONTEXT_CHARS: usize = 320;
/// Silence after which the previous context no longer describes what is being said.
pub const CONTEXT_TTL: Duration = Duration::from_secs(90);
/// Segments shorter than this (characters, trimmed) carry no useful context ("ừ", "ok").
const MIN_SEGMENT_CHARS: usize = 2;

#[derive(Debug, Default)]
pub struct RollingContext {
    tail: String,
    updated_at: Option<Instant>,
    /// Bumped on every change, so the writer knows when to resend the prompt.
    version: u64,
}

impl RollingContext {
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Appends a final transcript segment.
    pub fn push(&mut self, text: &str, now: Instant) {
        self.expire(now);
        let text = normalize_ws(text);
        if text.chars().count() < MIN_SEGMENT_CHARS {
            return;
        }
        // Early sentences and the remainder of a turn never overlap, but a model that loops
        // re-emits the same sentence: keep it once so it cannot dominate the prompt.
        if self.tail.ends_with(&text) {
            self.updated_at = Some(now);
            return;
        }
        if !self.tail.is_empty() {
            self.tail.push(' ');
        }
        self.tail.push_str(&text);
        self.tail = keep_tail(&self.tail, MAX_CONTEXT_CHARS);
        self.updated_at = Some(now);
        self.version += 1;
    }

    /// The current tail, empty once expired.
    pub fn snapshot(&mut self, now: Instant) -> String {
        self.expire(now);
        self.tail.clone()
    }

    fn expire(&mut self, now: Instant) {
        if let Some(at) = self.updated_at
            && now.saturating_duration_since(at) > CONTEXT_TTL
            && !self.tail.is_empty()
        {
            self.tail.clear();
            self.updated_at = None;
            self.version += 1;
        }
    }
}

/// The prompt for the next utterance: the static vocabulary hint first, then the tail.
pub fn compose_prompt(base: Option<&str>, tail: &str) -> Option<String> {
    let base = base.map(str::trim).filter(|b| !b.is_empty());
    let tail = tail.trim();
    match (base, tail.is_empty()) {
        (None, true) => None,
        (Some(b), true) => Some(b.to_string()),
        (None, false) => Some(tail.to_string()),
        (Some(b), false) => Some(format!("{b}\n{tail}")),
    }
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Last `max` characters of `s`, starting on a word boundary when one exists in the window.
fn keep_tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let cut: String = s.chars().skip(n - max).collect();
    match cut.find(' ') {
        Some(i) if i + 1 < cut.len() => cut[i + 1..].to_string(),
        _ => cut,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_versions() {
        let now = Instant::now();
        let mut c = RollingContext::default();
        c.push("Cái của em", now);
        c.push("nó chạy rồi nè.", now);
        assert_eq!(c.snapshot(now), "Cái của em nó chạy rồi nè.");
        assert_eq!(c.version(), 2);
    }

    #[test]
    fn skips_tiny_and_repeated_segments() {
        let now = Instant::now();
        let mut c = RollingContext::default();
        c.push(" ", now);
        c.push("ừ", now);
        c.push("Deploy xong rồi.", now);
        c.push("Deploy xong rồi.", now);
        assert_eq!(c.snapshot(now), "Deploy xong rồi.");
        assert_eq!(c.version(), 1);
    }

    #[test]
    fn keeps_a_bounded_tail_on_a_word_boundary_utf8_safe() {
        let now = Instant::now();
        let mut c = RollingContext::default();
        for _ in 0..100 {
            c.push("tiếng Việt có dấu", now);
        }
        let t = c.snapshot(now);
        assert!(t.chars().count() <= MAX_CONTEXT_CHARS);
        assert!(
            t.starts_with("tiếng")
                || t.starts_with("Việt")
                || t.starts_with("có")
                || t.starts_with("dấu")
        );
    }

    #[test]
    fn expires_after_long_silence() {
        let t0 = Instant::now();
        let mut c = RollingContext::default();
        c.push("Hello team.", t0);
        let later = t0 + CONTEXT_TTL + Duration::from_secs(1);
        assert_eq!(c.snapshot(later), "");
        assert_eq!(c.version(), 2);
    }

    #[test]
    fn composes_base_and_tail() {
        assert_eq!(compose_prompt(None, ""), None);
        assert_eq!(
            compose_prompt(Some(" WarpTalk "), ""),
            Some("WarpTalk".into())
        );
        assert_eq!(compose_prompt(None, "a b"), Some("a b".into()));
        assert_eq!(
            compose_prompt(Some("WarpTalk"), "a b"),
            Some("WarpTalk\na b".into())
        );
    }
}
