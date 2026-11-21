//! Sentence splitting for vi / en / ja (and any language using `. ! ?` or full-width `。！？`).
//!
//! Port of the upstream `SENTENCE_SPLIT_REGEX` without a regex dependency:
//! - Latin `. ! ?` end a sentence only when followed by whitespace (so `3.5`, `e.g.x` stay whole);
//! - full-width `。！？` end a sentence with NO whitespace needed, but not inside a run of
//!   terminators (`！？`) and not before a closing bracket or quote (`「はい。」` stays whole);
//! - newlines always split.
//!
//! The streaming variant additionally decides whether a terminator at the very end of the buffer
//! is final, because the next delta may continue it (`3.` + `5`, `。` + `」`).

const FULL_WIDTH_TERMINATORS: &[char] = &['。', '！', '？'];
const CLOSERS: &[char] = &['」', '』', '）', '】', '”', '’'];

fn is_latin_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

fn is_full_width_terminator(c: char) -> bool {
    FULL_WIDTH_TERMINATORS.contains(&c)
}

/// A Latin terminator at the end of the buffer is final unless it may be a decimal point or an
/// abbreviation dot: `!`/`?` always are; `.` is not after a digit or inside a dotted token.
fn latin_end_is_final(prefix_with_terminator: &str) -> bool {
    let Some(last) = prefix_with_terminator.chars().last() else {
        return false;
    };
    if last != '.' {
        return true;
    }
    let before = &prefix_with_terminator[..prefix_with_terminator.len() - 1];
    if before.chars().last().is_none_or(|c| c.is_ascii_digit()) {
        return false;
    }
    let word = before.rsplit(char::is_whitespace).next().unwrap_or("");
    !word.contains('.')
}

/// Splits `text` into the complete sentences and the unfinished tail.
///
/// With `final_text` the tail is flushed as a last sentence (the text will not grow). Without
/// it a terminator at the end only counts when it cannot be continued (see module docs).
pub fn split_stream(text: &str, final_text: bool) -> (Vec<String>, String) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut sentences = Vec::new();
    let mut start = 0usize; // byte offset of the current sentence
    let mut i = 0usize;
    let push = |s: &str, out: &mut Vec<String>| {
        let t = s.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    };
    while i < chars.len() {
        let (pos, c) = chars[i];
        let next = chars.get(i + 1).map(|&(_, n)| n);
        let end_of_c = pos + c.len_utf8();
        let mut cut_to: Option<usize> = None; // byte offset where the next sentence starts
        if c == '\n' || c == '\r' {
            push(&text[start..pos], &mut sentences);
            cut_to = Some(end_of_c);
        } else if is_latin_terminator(c) {
            match next {
                Some(n) if n.is_whitespace() => {
                    push(&text[start..end_of_c], &mut sentences);
                    cut_to = Some(end_of_c);
                }
                None if final_text || latin_end_is_final(&text[..end_of_c]) => {
                    push(&text[start..end_of_c], &mut sentences);
                    cut_to = Some(end_of_c);
                }
                _ => {}
            }
        } else if is_full_width_terminator(c) {
            match next {
                Some(n) if is_full_width_terminator(n) || CLOSERS.contains(&n) => {}
                Some(_) => {
                    push(&text[start..end_of_c], &mut sentences);
                    cut_to = Some(end_of_c);
                }
                None if final_text => {
                    push(&text[start..end_of_c], &mut sentences);
                    cut_to = Some(end_of_c);
                }
                None => {}
            }
        }
        i += 1;
        if let Some(offset) = cut_to {
            start = offset;
            // Swallow the whitespace after the boundary.
            while i < chars.len() && chars[i].1.is_whitespace() {
                start = chars[i].0 + chars[i].1.len_utf8();
                i += 1;
            }
        }
    }
    let tail = &text[start..];
    if final_text {
        push(tail, &mut sentences);
        (sentences, String::new())
    } else {
        (sentences, tail.trim_start().to_string())
    }
}

/// All sentences of a finished text.
pub fn split_sentences(text: &str) -> Vec<String> {
    split_stream(text, true).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_basic() {
        assert_eq!(
            split_sentences("Hello there. How are you? Fine!"),
            ["Hello there.", "How are you?", "Fine!"]
        );
    }

    #[test]
    fn decimals_and_dotted_tokens_do_not_split() {
        assert_eq!(split_sentences("Version 3.5 is out."), ["Version 3.5 is out."]);
        assert_eq!(split_sentences("see e.g.x now"), ["see e.g.x now"]);
    }

    #[test]
    fn vietnamese() {
        assert_eq!(
            split_sentences("Xin chào mọi người. Hôm nay chúng ta họp về dự án mới nhé? Được rồi."),
            [
                "Xin chào mọi người.",
                "Hôm nay chúng ta họp về dự án mới nhé?",
                "Được rồi."
            ]
        );
    }

    #[test]
    fn japanese_full_width_without_spaces() {
        assert_eq!(
            split_sentences("すみません。それは本当ですか？はい！"),
            ["すみません。", "それは本当ですか？", "はい！"]
        );
    }

    #[test]
    fn japanese_runs_and_closing_quotes_stay_together() {
        assert_eq!(split_sentences("本当！？そうです。"), ["本当！？", "そうです。"]);
        assert_eq!(
            split_sentences("彼は「はい。」と言った。次です。"),
            ["彼は「はい。」と言った。", "次です。"]
        );
    }

    #[test]
    fn newlines_split() {
        assert_eq!(split_sentences("one\ntwo\r\nthree"), ["one", "two", "three"]);
    }

    #[test]
    fn stream_holds_back_unfinished_tail() {
        let (done, tail) = split_stream("Hello there. How are", false);
        assert_eq!(done, ["Hello there."]);
        assert_eq!(tail, "How are");
    }

    #[test]
    fn stream_flushes_clean_latin_end_but_not_decimal() {
        let (done, tail) = split_stream("Hello there.", false);
        assert_eq!(done, ["Hello there."]);
        assert_eq!(tail, "");
        let (done, tail) = split_stream("It costs 3.", false);
        assert!(done.is_empty());
        assert_eq!(tail, "It costs 3.");
        let (done, _) = split_stream("Really?", false);
        assert_eq!(done, ["Really?"]);
        let (done, tail) = split_stream("Use e.g.", false);
        assert!(done.is_empty());
        assert_eq!(tail, "Use e.g.");
    }

    #[test]
    fn stream_full_width_waits_for_next_char() {
        let (done, tail) = split_stream("すみません。", false);
        assert!(done.is_empty());
        assert_eq!(tail, "すみません。");
        let (done, tail) = split_stream("すみません。それは", false);
        assert_eq!(done, ["すみません。"]);
        assert_eq!(tail, "それは");
        // The closing quote arriving later must not be orphaned.
        let (done, _) = split_stream("彼は「はい。」", false);
        assert!(done.is_empty());
    }

    #[test]
    fn empty_and_whitespace() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n ").is_empty());
    }
}
