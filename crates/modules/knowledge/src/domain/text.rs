//! Text primitives shared by chunking, PII scanning and lexical scoring: token estimates,
//! sentence splitting and tokenisation. Pure, no IO.

/// CJK ideographs, kana and hangul: one estimated token per character.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // hiragana, katakana
        | 0x31F0..=0x31FF
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xAC00..=0xD7AF // hangul syllables
        | 0xF900..=0xFAFF
        | 0xFF66..=0xFF9F // half-width katakana
    )
}

/// Rough, provider-independent token estimate (errs on the high side): each CJK character
/// counts as one token, every other word as `ceil(chars / 4)`.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0usize;
    let mut word_chars = 0usize;
    let flush = |tokens: &mut usize, word_chars: &mut usize| {
        if *word_chars > 0 {
            *tokens += (*word_chars).div_ceil(4);
            *word_chars = 0;
        }
    };
    for c in text.chars() {
        if is_cjk(c) {
            flush(&mut tokens, &mut word_chars);
            tokens += 1;
        } else if c.is_alphanumeric() {
            word_chars += 1;
        } else {
            flush(&mut tokens, &mut word_chars);
            if !c.is_whitespace() && !c.is_ascii_punctuation() {
                tokens += 1;
            }
        }
    }
    flush(&mut tokens, &mut word_chars);
    tokens
}

pub fn has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
}

const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "vs", "etc", "e.g", "i.e", "fig", "inc", "ltd",
    "co", "st", "approx", "dept", "est", "tp", "ths", "ts", "gs",
];

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')' | ']' | '」' | '』' | '）')
}

fn ends_with_abbreviation(buf: &str) -> bool {
    let word: String = buf
        .trim_end_matches('.')
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let lower = word.to_lowercase();
    if lower.is_empty() {
        return false;
    }
    // A single capital letter is an initial ("J. Smith").
    if word.chars().count() == 1 && word.chars().all(|c| c.is_uppercase()) {
        return true;
    }
    ABBREVIATIONS.contains(&lower.as_str())
}

fn is_list_marker(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("• ") || t.starts_with("・") {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && digits <= 3 && {
        let rest = &t[digits..];
        rest.starts_with(". ") || rest.starts_with(") ")
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a paragraph into sentences. Handles `. ! ? …` followed by whitespace, CJK
/// terminators (`。！？`), common abbreviations, initials, decimals and list items. Whitespace
/// inside a sentence is collapsed.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let push = |buf: &mut String, out: &mut Vec<String>| {
        let s = collapse_ws(buf);
        if !s.is_empty() {
            out.push(s);
        }
        buf.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            // A newline that starts a list item ends the previous sentence.
            let rest: String = chars[i + 1..].iter().take(8).collect();
            if is_list_marker(&rest) {
                push(&mut buf, &mut out);
            } else {
                buf.push(' ');
            }
            i += 1;
            continue;
        }
        buf.push(c);
        let cjk_end = matches!(c, '。' | '！' | '？');
        let latin_end = matches!(c, '.' | '!' | '?' | '…');
        if cjk_end || latin_end {
            // absorb run of terminators and closing quotes
            let mut j = i + 1;
            while j < chars.len()
                && (matches!(chars[j], '.' | '!' | '?' | '…') || is_closer(chars[j]))
            {
                buf.push(chars[j]);
                j += 1;
            }
            let at_end = j >= chars.len();
            let next_ws = !at_end && chars[j].is_whitespace();
            let boundary = if cjk_end {
                true
            } else if at_end || next_ws {
                !(c == '.' && ends_with_abbreviation(&buf))
            } else {
                false
            };
            if boundary {
                push(&mut buf, &mut out);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    push(&mut buf, &mut out);
    out
}

/// Split an over-long string at whitespace (or hard, for unbroken CJK) into pieces within
/// both the token and character budget. Only used for single sentences larger than the budget.
pub fn split_oversized(s: &str, max_tokens: usize, char_limit: usize) -> Vec<String> {
    let max_tokens = max_tokens.max(1);
    let budget = if has_cjk(s) {
        char_limit.min(max_tokens)
    } else {
        char_limit.min(max_tokens * 3)
    }
    .max(8);
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let hard_end = (start + budget).min(chars.len());
        let mut end = hard_end;
        if hard_end < chars.len() {
            // prefer a whitespace break in the second half of the window
            let min_end = start + budget / 2;
            if let Some(p) = (min_end..hard_end)
                .rev()
                .find(|&p| chars[p].is_whitespace())
            {
                end = p;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim().to_owned();
        if !piece.is_empty() {
            out.push(piece);
        }
        start = end;
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }
    }
    out
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "was", "were", "be",
    "it", "this", "that", "with", "as", "at", "by", "from", "what", "which", "who", "how", "do",
    "does", "did", "i", "we", "you", "they", "he", "she", "about",
];

/// Lowercased lexical tokens: alphanumeric words (stopwords and 1-char ASCII dropped), CJK
/// text as character bigrams (single char when isolated).
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut cjk_run: Vec<char> = Vec::new();
    let flush_word = |word: &mut String, out: &mut Vec<String>| {
        if !word.is_empty() {
            let w = std::mem::take(word);
            if !(w.is_ascii() && w.len() < 2) && !STOPWORDS.contains(&w.as_str()) {
                out.push(w);
            }
        }
    };
    let flush_cjk = |run: &mut Vec<char>, out: &mut Vec<String>| {
        match run.len() {
            0 => {}
            1 => out.push(run[0].to_string()),
            _ => {
                for w in run.windows(2) {
                    out.push(w.iter().collect());
                }
            }
        }
        run.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut out);
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut out);
            word.extend(c.to_lowercase());
        } else {
            flush_word(&mut word, &mut out);
            flush_cjk(&mut cjk_run, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_cjk(&mut cjk_run, &mut out);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_counts_cjk_per_char() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello world"), 4);
        assert_eq!(estimate_tokens("会議の議事録"), 6);
        assert!(estimate_tokens("Xin chào các bạn") >= 4);
    }

    #[test]
    fn sentences_respect_abbreviations_decimals_and_cjk() {
        let s = split_sentences("Dr. Smith paid 3.5 million. J. Doe agreed! Is it done? Yes.");
        assert_eq!(
            s,
            [
                "Dr. Smith paid 3.5 million.",
                "J. Doe agreed!",
                "Is it done?",
                "Yes."
            ]
        );
        let j = split_sentences("会議を始めます。議題は予算です。");
        assert_eq!(j, ["会議を始めます。", "議題は予算です。"]);
        let v = split_sentences("Chúng ta họp lúc 9h. Mọi người đến đúng giờ nhé.");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn list_items_are_separate_sentences() {
        let s = split_sentences("Agenda:\n- budget review\n- hiring plan\n1. wrap up");
        assert_eq!(
            s,
            ["Agenda:", "- budget review", "- hiring plan", "1. wrap up"]
        );
    }

    #[test]
    fn oversized_splits_within_budget() {
        let s = "word ".repeat(200);
        let parts = split_oversized(s.trim(), 20, 2000);
        assert!(parts.len() > 3);
        assert!(
            parts
                .iter()
                .all(|p| estimate_tokens(p) <= 40 && !p.is_empty())
        );
        let cjk = "あ".repeat(100);
        let parts = split_oversized(&cjk, 30, 2000);
        assert!(parts.iter().all(|p| p.chars().count() <= 30));
        assert_eq!(parts.iter().map(|p| p.chars().count()).sum::<usize>(), 100);
    }

    #[test]
    fn tokenizer_drops_stopwords_and_bigrams_cjk() {
        assert_eq!(tokenize("The Budget of 2026!"), ["budget", "2026"]);
        assert_eq!(tokenize("予算会議"), ["予算", "算会", "会議"]);
    }
}
