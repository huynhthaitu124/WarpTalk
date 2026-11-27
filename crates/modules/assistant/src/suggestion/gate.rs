//! Decide gate: spends the stage-1 (decide) call only when the conversation moved enough.
//!
//! Without it every segment that passes stage 0 costs one decide call over the whole window,
//! and a quiet stretch of small talk (where decide keeps declining) costs the most. The gate
//! keeps per-room counters and lets a decide through only when:
//! - at least `min_new_words` words arrived since the previous decide (the window then holds
//!   new content worth judging; smaller pieces are judged together next time),
//! - `min_interval_seconds` passed since the previous decide (fast speech yields many short
//!   segments),
//! - and, after `n` consecutive declines, `min_new_words * backoff_factor^n` words arrived
//!   (capped at `max_new_words`): the longer nothing is suggestion-worthy, the less often we ask.
//!
//! A segment that asks the user for a response skips the gate (an `answer` is latency-critical).
//! A positive decision or a published suggestion resets the backoff.

use super::config::DecideGateConfig;

#[derive(Debug, Default, Clone)]
pub struct DecideGate {
    words_since: u32,
    last_decide_ms: Option<i64>,
    declines: u32,
}

impl DecideGate {
    /// Counts the words of a segment that entered the window.
    pub fn observe(&mut self, words: u32) {
        self.words_since = self.words_since.saturating_add(words);
    }

    /// Whether a decide call is allowed now.
    pub fn allows(&self, cfg: &DecideGateConfig, now_ms: i64, urgent: bool) -> bool {
        if urgent || !cfg.enabled {
            return true;
        }
        if let Some(at) = self.last_decide_ms
            && now_ms.saturating_sub(at) < i64::from(cfg.min_interval_seconds) * 1000
        {
            return false;
        }
        self.words_since >= self.required_words(cfg)
    }

    /// Words needed before the next (non-urgent) decide.
    pub fn required_words(&self, cfg: &DecideGateConfig) -> u32 {
        let base = cfg.min_new_words as f32;
        let grown = base
            * cfg
                .backoff_factor
                .max(1.0)
                .powi(self.declines.min(16) as i32);
        (grown.round() as u32).min(cfg.max_new_words.max(cfg.min_new_words))
    }

    /// Records that a decide call is being made.
    pub fn decided(&mut self, now_ms: i64) {
        self.words_since = 0;
        self.last_decide_ms = Some(now_ms);
    }

    /// Records the decide verdict: declines (and failures) back off, a yes resets.
    pub fn verdict(&mut self, suggested: bool) {
        if suggested {
            self.declines = 0;
        } else {
            self.declines = self.declines.saturating_add(1);
        }
    }
}

/// Words in a segment; CJK scripts (written without spaces) count one word per character.
pub fn word_count(text: &str) -> u32 {
    text.split_whitespace()
        .map(|w| {
            let cjk = w
                .chars()
                .filter(|c| matches!(*c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}'))
                .count();
            cjk.max(1) as u32
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> DecideGateConfig {
        DecideGateConfig {
            enabled: true,
            min_new_words: 8,
            max_new_words: 48,
            backoff_factor: 2.0,
            min_interval_seconds: 4,
        }
    }

    #[test]
    fn counts_words_across_scripts() {
        assert_eq!(word_count("anh xem đi"), 3);
        assert_eq!(word_count("会議です ok"), 5);
    }

    #[test]
    fn waits_for_enough_new_words() {
        let mut g = DecideGate::default();
        g.observe(5);
        assert!(!g.allows(&cfg(), 0, false));
        g.observe(3);
        assert!(g.allows(&cfg(), 0, false));
    }

    #[test]
    fn min_interval_holds_even_with_many_words() {
        let mut g = DecideGate::default();
        g.decided(10_000);
        g.observe(30);
        assert!(!g.allows(&cfg(), 12_000, false));
        assert!(g.allows(&cfg(), 14_000, false));
    }

    #[test]
    fn declines_back_off_and_a_yes_resets() {
        let mut g = DecideGate::default();
        let c = cfg();
        assert_eq!(g.required_words(&c), 8);
        g.verdict(false);
        assert_eq!(g.required_words(&c), 16);
        g.verdict(false);
        assert_eq!(g.required_words(&c), 32);
        g.verdict(false);
        g.verdict(false);
        assert_eq!(g.required_words(&c), 48);
        g.verdict(true);
        assert_eq!(g.required_words(&c), 8);
    }

    #[test]
    fn urgent_or_disabled_always_passes() {
        let mut g = DecideGate::default();
        g.decided(0);
        assert!(g.allows(&cfg(), 0, true));
        let off = DecideGateConfig {
            enabled: false,
            ..cfg()
        };
        assert!(g.allows(&off, 0, false));
    }
}
