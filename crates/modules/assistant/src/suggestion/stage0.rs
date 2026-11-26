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

/// True when the segment looks like it asks somebody for a reply, information or a decision: a
/// question mark anywhere, or a configured ask phrase on whole words anywhere. Deliberately
/// generous; it only prioritises a segment for the `answer` fast path, the model decides.
pub fn asks_for_response(text: &str, h: &HeuristicLang) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if h.question_marks.iter().any(|m| t.contains(m.as_str())) {
        return true;
    }
    let ws = words(t);
    h.ask_phrases.iter().any(|p| {
        let pw = words(p);
        !pw.is_empty() && ws.windows(pw.len()).any(|w| w == pw.as_slice())
    })
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
    let floor = if looks_like_question(text, &h) || asks_for_response(text, &h) {
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
    fn vietnamese_and_english_asks_are_detected() {
        let c = cfg();
        let h = c.heuristics_for("vi");
        for t in [
            "Bao giờ bên anh gửi báo giá được?",
            "khi nào team deploy xong",
            "Anh có thể gửi file hôm nay được không",
            "Báo cáo xong chưa",
            "Ai sẽ phụ trách phần này",
            "Sao lại chậm vậy",
            "Có thể confirm giúp em deadline không?",
        ] {
            assert!(
                asks_for_response(t, &h) || looks_like_question(t, &h),
                "{t}"
            );
        }
        assert!(!asks_for_response("Hôm nay trời đẹp quá", &h));
        // Whole words only: "ai" inside "mai" must not match.
        let phrases = HeuristicLang {
            ask_phrases: vec!["ai".into()],
            ..Default::default()
        };
        assert!(!asks_for_response("ngày mai đi", &phrases));
        let en = c.heuristics_for("en");
        assert!(asks_for_response("Could you send the numbers", &en));
        assert!(asks_for_response("so the launch date is?", &en));
        assert!(!asks_for_response("we ship on Friday", &en));
        // A short ask gets the low question floor.
        assert_eq!(check(&c, "khi nào xong", "vi", None), Stage0::Pass);
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
