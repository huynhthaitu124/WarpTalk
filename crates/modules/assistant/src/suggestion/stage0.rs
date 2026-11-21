//! Stage 0: local heuristics, no tokens spent (spec 09 section 4).

use super::config::{HeuristicLang, SuggestionConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage0 {
    Pass,
    Reject(&'static str),
}

fn normalise_word(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalise_word)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Deliberately generous: only selects the word-count floor, never decides to suggest.
pub fn looks_like_question(text: &str, h: &HeuristicLang) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if h.question_marks.iter().any(|m| t.ends_with(m.as_str())) {
        return true;
    }
    let ws = words(t);
    match (ws.first(), ws.last()) {
        (Some(first), Some(last)) => h
            .question_words
            .iter()
            .any(|q| q.to_lowercase() == *first || q.to_lowercase() == *last),
        _ => false,
    }
}

/// `stt_confidence` is an average token logprob (<= 0); `None` or the configured sentinel means
/// "unknown" and always passes (WT-543).
pub fn check(
    cfg: &SuggestionConfig,
    text: &str,
    language: &str,
    stt_confidence: Option<f64>,
) -> Stage0 {
    let text = text.trim();
    if text.is_empty() {
        return Stage0::Reject("empty");
    }
    let h = cfg.heuristics_for(language);
    let floor = if looks_like_question(text, &h) {
        cfg.min_question_words
    } else {
        cfg.min_words
    } as usize;
    let ws = words(text);
    if ws.len() < floor {
        return Stage0::Reject("too_short");
    }
    if !h.filler_words.is_empty() {
        let filler: Vec<String> = h.filler_words.iter().map(|w| w.to_lowercase()).collect();
        if ws.iter().all(|w| filler.contains(w)) {
            return Stage0::Reject("filler");
        }
    }
    if let Some(c) = stt_confidence {
        let unknown = (c - f64::from(cfg.stt_unknown_sentinel)).abs() < 1e-9;
        if !unknown && c < f64::from(cfg.min_stt_confidence) {
            return Stage0::Reject("low_stt_confidence");
        }
    }
    Stage0::Pass
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cfg() -> SuggestionConfig {
        SuggestionConfig::default()
    }

    #[test]
    fn rejects_empty_short_and_filler() {
        let c = cfg();
        assert_eq!(check(&c, "  ", "en", None), Stage0::Reject("empty"));
        assert_eq!(
            check(&c, "ok then", "en", None),
            Stage0::Reject("too_short")
        );
        assert_eq!(
            check(&c, "yeah yeah okay sure right", "en", None),
            Stage0::Reject("filler")
        );
        assert_eq!(
            check(&c, "ừ dạ vâng rồi nhé", "vi", None),
            Stage0::Reject("filler")
        );
        assert_eq!(
            check(&c, "we ship the beta on Friday", "en", None),
            Stage0::Pass
        );
    }

    #[test]
    fn questions_use_their_own_floor() {
        let c = cfg();
        assert_eq!(check(&c, "JavaScript là gì?", "vi", None), Stage0::Pass);
        assert_eq!(check(&c, "what is Next", "en", None), Stage0::Pass);
        assert_eq!(
            check(&c, "Next.js now", "en", None),
            Stage0::Reject("too_short")
        );
    }

    #[test]
    fn stt_confidence_sentinel_passes() {
        let c = cfg();
        let t = "we ship the beta on Friday";
        assert_eq!(check(&c, t, "en", Some(-1.0)), Stage0::Pass);
        assert_eq!(check(&c, t, "en", None), Stage0::Pass);
        assert_eq!(
            check(&c, t, "en", Some(-0.9)),
            Stage0::Reject("low_stt_confidence")
        );
        assert_eq!(check(&c, t, "en", Some(-0.1)), Stage0::Pass);
    }

    #[test]
    fn thresholds_follow_config() {
        let mut c = cfg();
        c.min_words = 2;
        assert_eq!(check(&c, "ship Friday", "en", None), Stage0::Pass);
        c.min_words = 8;
        assert_eq!(
            check(&c, "we ship the beta on Friday", "en", None),
            Stage0::Reject("too_short")
        );
    }
}
