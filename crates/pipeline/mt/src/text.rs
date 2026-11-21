//! Text helpers: sentence splitting and the normalised key used by the speculative cache.

const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "vs", "etc", "e.g", "i.e", "st", "no", "inc",
    "ltd", "co",
];

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…')
}

fn is_cjk_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？')
}

/// Splits a transcript into sentences. A `.`/`!`/`?` ends a sentence when followed by
/// whitespace or the end of text; the CJK full stops (`。！？`) end one without whitespace.
/// Decimals ("3.5"), abbreviations ("Dr. Smith") and a stray lowercase continuation are held
/// back. Never returns empty pieces; concatenating the pieces with spaces preserves the words.
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        cur.push(c);
        if is_terminator(c) {
            // Swallow a run of terminators and closing quotes/brackets ("?!", "...", ".\"").
            while i + 1 < chars.len()
                && (is_terminator(chars[i + 1]) || matches!(chars[i + 1], '"' | '\'' | ')' | '”'))
            {
                i += 1;
                cur.push(chars[i]);
            }
            let next = chars.get(i + 1).copied();
            let boundary = match next {
                None => true,
                Some(n) if is_cjk_terminator(c) => !n.is_alphanumeric() || n.is_whitespace() || true,
                Some(n) if n.is_whitespace() => !holds_back(&cur, &chars[i + 1..]),
                Some(_) => false,
            };
            if boundary {
                let s = cur.trim().to_string();
                if !s.is_empty() {
                    out.push(s);
                }
                cur.clear();
            }
        }
        i += 1;
    }
    let tail = cur.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// After a `.` followed by whitespace: is this an abbreviation, not a sentence end?
fn holds_back(cur: &str, rest: &[char]) -> bool {
    if !cur.ends_with('.') || cur.ends_with("..") {
        return false;
    }
    let word: String = cur
        .trim_end_matches('.')
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '.')
        .to_lowercase();
    let next_lower = rest
        .iter()
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| c.is_lowercase());
    ABBREVIATIONS.contains(&word.as_str()) || (next_lower && word.chars().count() == 1)
}

/// Key for the speculative cache: casefolded, whitespace collapsed, trailing punctuation off.
/// Prefetch hints (growing deltas) and the validated segment differ in exactly that.
pub fn normalize_key(text: &str) -> String {
    let folded = text.to_lowercase();
    let collapsed = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(|c: char| is_terminator(c) || c == ' ')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_basic_and_keeps_decimals_and_abbreviations() {
        assert_eq!(
            split_sentences("Hello there. How are you? Fine!"),
            ["Hello there.", "How are you?", "Fine!"]
        );
        assert_eq!(
            split_sentences("It costs 3.5 dollars. Ask Dr. Smith today."),
            ["It costs 3.5 dollars.", "Ask Dr. Smith today."]
        );
        assert_eq!(split_sentences("no terminator"), ["no terminator"]);
        assert!(split_sentences("   ").is_empty());
    }

    #[test]
    fn splits_cjk_without_spaces_and_runs() {
        assert_eq!(
            split_sentences("明日リリースします。確認お願いします！"),
            ["明日リリースします。", "確認お願いします！"]
        );
        assert_eq!(split_sentences("Really?! Yes..."), ["Really?!", "Yes..."]);
    }

    #[test]
    fn key_ignores_case_space_and_trailing_punct() {
        assert_eq!(normalize_key("  Hello   World. "), normalize_key("hello world"));
        assert_ne!(normalize_key("hello"), normalize_key("hello world"));
    }
}
