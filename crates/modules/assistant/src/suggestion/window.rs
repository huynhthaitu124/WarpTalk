//! Rolling per-room transcript window, bounded by turns, age and estimated tokens.
//! All three limits come from the configuration current at the time of the call, so a hot
//! reload applies to the very next segment.

use std::collections::VecDeque;

use super::config::SuggestionConfig;
use super::model::Turn;

/// Cheap token estimate (no tokenizer): `ceil(chars / chars_per_token)`, at least 1.
pub fn estimate_tokens(text: &str, chars_per_token: f32) -> u32 {
    let chars = text.chars().count() as f32;
    ((chars / chars_per_token.max(1.0)).ceil() as u32).max(1)
}

#[derive(Debug, Default)]
pub struct RollingWindow {
    turns: VecDeque<Turn>,
}

impl RollingWindow {
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Append `turn` and evict by count, age and tokens.
    pub fn push(&mut self, turn: Turn, cfg: &SuggestionConfig, now_ms: i64) {
        self.turns.push_back(turn);
        self.trim(cfg, now_ms);
    }

    /// Evict, keeping at least the newest turn.
    pub fn trim(&mut self, cfg: &SuggestionConfig, now_ms: i64) {
        // The current segment counts as one of the window's turns and is excluded from the
        // context later, so the window holds `window_size` turns of context plus the subject.
        let max_turns = cfg.window_size as usize + 1;
        while self.turns.len() > max_turns {
            self.turns.pop_front();
        }
        if cfg.window.max_age_seconds > 0 {
            let cutoff = now_ms - i64::from(cfg.window.max_age_seconds) * 1000;
            while self.turns.len() > 1 && self.turns.front().is_some_and(|t| t.at_ms < cutoff) {
                self.turns.pop_front();
            }
        }
        if cfg.window.max_tokens > 0 {
            let mut total: u64 = self.turns.iter().map(|t| u64::from(t.tokens)).sum();
            while self.turns.len() > 1 && total > u64::from(cfg.window.max_tokens) {
                if let Some(t) = self.turns.pop_front() {
                    total -= u64::from(t.tokens);
                }
            }
        }
    }

    /// All turns except the newest (the subject), oldest first.
    pub fn context(&self) -> Vec<Turn> {
        let n = self.turns.len().saturating_sub(1);
        self.turns.iter().take(n).cloned().collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn turn(i: i64, text: &str, cpt: f32) -> Turn {
        Turn {
            segment_id: format!("s{i}"),
            speaker_id: "u".into(),
            text: text.into(),
            language: "en".into(),
            at_ms: i * 1000,
            tokens: estimate_tokens(text, cpt),
        }
    }

    #[test]
    fn bounded_by_turns() {
        let cfg = SuggestionConfig {
            window_size: 3,
            ..Default::default()
        };
        let mut w = RollingWindow::default();
        for i in 0..10 {
            w.push(turn(i, "hello there", 4.0), &cfg, i * 1000);
        }
        assert_eq!(w.context().len(), 3);
        assert_eq!(w.context()[0].segment_id, "s6");
    }

    #[test]
    fn bounded_by_age() {
        let mut cfg = SuggestionConfig::default();
        cfg.window.max_age_seconds = 5;
        let mut w = RollingWindow::default();
        for i in 0..10 {
            w.push(turn(i, "hello there", 4.0), &cfg, i * 1000);
        }
        // now = 9s; cutoff = 4s -> keeps s4..s9 (5 context + subject)
        assert_eq!(w.context().first().unwrap().segment_id, "s4");
    }

    #[test]
    fn bounded_by_tokens_and_keeps_newest() {
        let mut cfg = SuggestionConfig::default();
        cfg.window.max_tokens = 10;
        let mut w = RollingWindow::default();
        for i in 0..6 {
            w.push(turn(i, &"x".repeat(16), 4.0), &cfg, 0); // 4 tokens each
        }
        assert_eq!(w.len(), 2);
        cfg.window.max_tokens = 1;
        w.trim(&cfg, 0);
        assert_eq!(w.len(), 1);
        assert!(w.context().is_empty());
    }
}
