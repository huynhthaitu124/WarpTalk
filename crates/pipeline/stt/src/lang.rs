//! Vietnamese / English language detection for code-switched speech (copilot `auto` / `multi`
//! language modes). A deliberately small, dependency-free heuristic: Vietnamese is recognised
//! by its diacritics (and, for text written without them, by a short list of very frequent
//! function words), English by its function words. It decides per *segment*, never per word.
//!
//! The thresholds live in [`LangDetectConfig`] (deserializable) so they can be tuned without a
//! release; the defaults were chosen on short meeting sentences and are conservative: when the
//! evidence is thin the provider's own answer (or the first declared language) wins.

use serde::Deserialize;

/// Tunables of [`detect`] / [`resolve_language`].
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LangDetectConfig {
    /// A text is `Vi` when at least this share of the evidence points to Vietnamese ...
    pub vi_min_share: f32,
    /// ... and `En` when at most this share does. In between it is `Mixed`.
    pub en_max_share: f32,
    /// Evidence weight of an ASCII word that is in neither stop list (English technical terms
    /// are mostly such words, but so are a few Vietnamese words written without diacritics).
    pub unknown_ascii_weight: f32,
    /// Below this much total evidence the text is `Unknown`.
    pub min_evidence: f32,
    /// Mixed text needs at least this much evidence on each side to be called `Mixed`.
    pub mixed_min_side: f32,
}

impl Default for LangDetectConfig {
    fn default() -> Self {
        Self {
            vi_min_share: 0.65,
            en_max_share: 0.2,
            unknown_ascii_weight: 0.5,
            min_evidence: 1.0,
            mixed_min_side: 2.0,
        }
    }
}

/// Which side dominates a mixed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dominant {
    Vi,
    En,
}

/// Outcome of [`detect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    Vi,
    En,
    /// Both languages are clearly present (code-switching); `Dominant` is the larger side.
    Mixed(Dominant),
    Unknown,
}

impl Detected {
    /// The single language to store for the segment (`None` when `Unknown`).
    pub fn language(self) -> Option<&'static str> {
        match self {
            Detected::Vi | Detected::Mixed(Dominant::Vi) => Some("vi"),
            Detected::En | Detected::Mixed(Dominant::En) => Some("en"),
            Detected::Unknown => None,
        }
    }
}

/// Characters only Vietnamese uses (in a vi/en context): the modified base letters, the tone
/// marked vowels of Latin-1 and the precomposed block U+1EA0..U+1EF9, and any combining mark.
fn is_vi_marked(c: char) -> bool {
    matches!(
        c,
        'ă' | 'â'
            | 'đ'
            | 'ê'
            | 'ô'
            | 'ơ'
            | 'ư'
            | 'Ă'
            | 'Â'
            | 'Đ'
            | 'Ê'
            | 'Ô'
            | 'Ơ'
            | 'Ư'
            | 'à'
            | 'á'
            | 'ã'
            | 'è'
            | 'é'
            | 'ì'
            | 'í'
            | 'ò'
            | 'ó'
            | 'õ'
            | 'ù'
            | 'ú'
            | 'ý'
            | 'À'
            | 'Á'
            | 'Ã'
            | 'È'
            | 'É'
            | 'Ì'
            | 'Í'
            | 'Ò'
            | 'Ó'
            | 'Õ'
            | 'Ù'
            | 'Ú'
            | 'Ý'
            | '\u{0300}'..='\u{036F}'
            | '\u{1EA0}'..='\u{1EF9}'
    )
}

/// Frequent Vietnamese words that are plain ASCII (text without diacritics, or with them
/// stripped by an upstream step).
const VI_ASCII: &[&str] = &[
    "toi", "ta", "anh", "chi", "em", "ban", "minh", "la", "va", "cua", "cho", "khong", "co", "duoc",
    "nhung", "mot", "cac", "nay", "do", "nhu", "voi", "trong", "de", "se", "da", "dang", "rat",
    "roi", "thi", "ma", "nen", "vi", "neu", "can", "phai", "lam", "gi", "sao", "nao", "ai", "hay",
    "cung", "con", "nguoi", "ngay", "nam", "xin", "chao", "cam", "on", "vang", "oi",
];

/// Frequent English function words.
const EN_STOP: &[&str] = &[
    "the", "and", "is", "are", "was", "were", "be", "been", "to", "of", "in", "on", "at", "for",
    "with", "this", "that", "these", "those", "it", "its", "we", "you", "they", "he", "she", "i",
    "my", "our", "your", "their", "have", "has", "had", "do", "does", "did", "not", "but", "or",
    "if", "so", "will", "would", "can", "could", "should", "just", "about", "from", "by", "as",
    "an", "a", "what", "when", "where", "which", "who", "how", "there", "here", "let", "lets",
    "let's", "ok", "okay", "yes", "no", "yeah", "thanks", "please", "need", "want", "think",
    "going", "get", "make", "then", "also", "after", "before", "more", "all", "some", "any",
];

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphabetic() || c == '\'' || is_vi_marked(c)))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// Classify one segment's text.
pub fn detect(text: &str, cfg: &LangDetectConfig) -> Detected {
    let (mut vi, mut en) = (0.0f32, 0.0f32);
    for w in words(text) {
        if w.chars().any(is_vi_marked) {
            vi += 1.0;
        } else if EN_STOP.contains(&w.as_str()) {
            // "do" / "an" etc. are in both lists conceptually; English wins only when the word
            // is not also a very frequent Vietnamese one.
            if VI_ASCII.contains(&w.as_str()) {
                vi += 0.5;
                en += 0.5;
            } else {
                en += 1.0;
            }
        } else if VI_ASCII.contains(&w.as_str()) {
            vi += 1.0;
        } else if w.is_ascii() && w.len() >= 3 && w.bytes().all(|b| b.is_ascii_alphabetic()) {
            en += cfg.unknown_ascii_weight;
        }
    }
    let total = vi + en;
    if total < cfg.min_evidence {
        return Detected::Unknown;
    }
    let share = vi / total;
    if share >= cfg.vi_min_share {
        return Detected::Vi;
    }
    if share <= cfg.en_max_share {
        return Detected::En;
    }
    let dominant = if share >= 0.5 {
        Dominant::Vi
    } else {
        Dominant::En
    };
    if vi >= cfg.mixed_min_side && en >= cfg.mixed_min_side {
        Detected::Mixed(dominant)
    } else {
        match dominant {
            Dominant::Vi => Detected::Vi,
            Dominant::En => Detected::En,
        }
    }
}

fn base(lang: &str) -> String {
    lang.split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Language to store on a segment.
///
/// * `provider`: what the STT reported (may be the hint that was sent, `auto`, `und`, empty).
/// * `allowed`: the session's declared languages (base codes, first = primary).
/// * `trust_provider`: when `true` (mode `auto`) a provider language that is in `allowed` is
///   kept as is; when `false` (mode `multi`) the heuristic may override it if it is confident
///   (`Vi`/`En`/`Mixed`) and disagrees, because a hint-less provider often just echoes a
///   default.
///
/// Languages other than `vi`/`en` are never produced by the heuristic: for them the provider's
/// answer is returned unchanged.
pub fn resolve_language(
    text: &str,
    provider: Option<&str>,
    allowed: &[String],
    trust_provider: bool,
    cfg: &LangDetectConfig,
) -> String {
    let prov = provider
        .map(base)
        .filter(|p| !p.is_empty() && p != "auto" && p != "und" && p != "unknown");
    let in_allowed = |l: &str| allowed.is_empty() || allowed.iter().any(|a| base(a) == l);
    let can_detect = |l: &str| in_allowed(l) || allowed.is_empty();
    if let Some(p) = &prov {
        if p != "vi" && p != "en" && in_allowed(p) {
            return p.clone();
        }
        if trust_provider && in_allowed(p) {
            return p.clone();
        }
    }
    match detect(text, cfg).language() {
        Some(l) if can_detect(l) => l.to_owned(),
        _ => prov
            .or_else(|| allowed.first().map(|a| base(a)))
            .unwrap_or_else(|| "und".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LangDetectConfig {
        LangDetectConfig::default()
    }

    fn vi_en() -> Vec<String> {
        vec!["vi".into(), "en".into()]
    }

    #[test]
    fn vietnamese_with_diacritics() {
        assert_eq!(
            detect("Chúng ta cần hoàn thành báo cáo trước thứ sáu", &cfg()),
            Detected::Vi
        );
    }

    #[test]
    fn vietnamese_without_diacritics() {
        assert_eq!(
            detect("toi khong biet anh co the lam duoc khong", &cfg()),
            Detected::Vi
        );
    }

    #[test]
    fn english() {
        assert_eq!(
            detect("Let's review the roadmap and then we can ship it", &cfg()),
            Detected::En
        );
    }

    #[test]
    fn code_switched_vietnamese_with_english_terms_stays_vietnamese() {
        let d = detect("Chúng ta cần deploy lên production vào chiều nay", &cfg());
        assert_eq!(d.language(), Some("vi"));
    }

    #[test]
    fn genuinely_mixed_is_reported_mixed_with_a_dominant_side() {
        let d = detect(
            "Ok let's sync on the roadmap, sau đó chúng ta release bản mới",
            &cfg(),
        );
        assert!(matches!(d, Detected::Mixed(_)), "{d:?}");
        assert!(d.language().is_some());
    }

    #[test]
    fn numbers_and_symbols_are_unknown() {
        assert_eq!(detect("12:30 - 4.5%", &cfg()), Detected::Unknown);
        assert_eq!(detect("", &cfg()), Detected::Unknown);
    }

    #[test]
    fn resolve_auto_keeps_a_valid_provider_language() {
        let l = resolve_language("hello there", Some("vi"), &vi_en(), true, &cfg());
        assert_eq!(l, "vi");
    }

    #[test]
    fn resolve_multi_overrides_a_wrong_provider_language() {
        let l = resolve_language(
            "Tôi nghĩ chúng ta nên bắt đầu",
            Some("en"),
            &vi_en(),
            false,
            &cfg(),
        );
        assert_eq!(l, "vi");
    }

    #[test]
    fn resolve_fills_in_a_missing_language() {
        assert_eq!(
            resolve_language("see you tomorrow then", Some("auto"), &vi_en(), false, &cfg()),
            "en"
        );
        assert_eq!(
            resolve_language("cảm ơn mọi người", None, &vi_en(), false, &cfg()),
            "vi"
        );
    }

    #[test]
    fn resolve_unknown_falls_back_to_primary() {
        assert_eq!(
            resolve_language("1 2 3", None, &vi_en(), false, &cfg()),
            "vi"
        );
    }

    #[test]
    fn other_languages_pass_through() {
        let allowed = vec!["vi".to_string(), "ja".to_string()];
        assert_eq!(
            resolve_language("こんにちは", Some("ja"), &allowed, false, &cfg()),
            "ja"
        );
    }
}
