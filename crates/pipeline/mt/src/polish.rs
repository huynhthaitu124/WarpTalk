//! Same-language tidy-up without letting the model rewrite the transcript.
//!
//! A repair tool this is NOT: a misheard word is gone, and an LLM asked to "clean up" a garbled
//! line produces fluent, confident, invented text that then flows into summaries and the
//! knowledge index. So the model is told what it may do and the result is verified
//! mechanically: every word returned must already be in the original, in the same order (a
//! subsequence), at most half may be dropped, and no number or negation may be deleted. A
//! failed check keeps the original.

pub fn instruction() -> String {
    "You are formatting a live meeting transcript. The text is already in the language your \
     output must use - do NOT translate it.\nYou may: fix capitalisation, add or correct \
     punctuation, fix spacing, and delete filler sounds, stutters and repeated false starts.\n\
     You may NOT: change any word, add any word, reorder words, complete an unfinished sentence, \
     or guess what a garbled passage was meant to say. Leave anything you cannot read exactly as \
     it is - a transcript that looks wrong is more useful than one that reads well and is \
     wrong.\nIf the text is already well formatted, return it unchanged. Return only the \
     corrected text, with no commentary and no quotation marks."
        .to_string()
}

const MAX_DELETED_FRACTION: f32 = 0.5;

/// Negation words whose deletion flips the meaning (kept short: the subsequence rule already
/// forbids substitution, this only guards deletions).
const NEGATIONS: &[&str] = &[
    "not",
    "no",
    "never",
    "n't",
    "without",
    "khong",
    "không",
    "chưa",
    "chua",
    "đừng",
    "dung",
    "ない",
    "ません",
    "不",
    "没",
    "沒",
    "没有",
    "안",
    "못",
];

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '\'' && c != '%'))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn is_negation(t: &str) -> bool {
    NEGATIONS.contains(&t) || t.ends_with("n't")
}

/// Whether `polished` is `original` with only formatting changed.
pub fn is_faithful(original: &str, polished: &str) -> bool {
    if polished.trim().is_empty() {
        return false;
    }
    let (o, p) = (tokens(original), tokens(polished));
    if o.is_empty() {
        return false;
    }
    // Subsequence walk; remember what was skipped.
    let mut skipped = Vec::new();
    let mut pi = 0;
    for tok in &o {
        if pi < p.len() && *tok == p[pi] {
            pi += 1;
        } else {
            skipped.push(tok);
        }
    }
    if pi != p.len() {
        return false; // something inserted, changed or reordered
    }
    if skipped
        .iter()
        .any(|t| t.chars().any(|c| c.is_ascii_digit()) || is_negation(t))
    {
        return false;
    }
    (skipped.len() as f32) <= o.len() as f32 * MAX_DELETED_FRACTION
}

/// The text to publish: the tidy version when faithful, otherwise the original. The single place
/// the decision is made, so no caller can publish an unchecked model output.
pub fn choose_transcript(original: &str, polished: &str) -> String {
    if is_faithful(original, polished) {
        polished.trim().to_string()
    } else {
        original.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_formatting_and_filler_removal() {
        assert!(is_faithful("so um we ship friday", "So, we ship Friday."));
        assert_eq!(
            choose_transcript("um we ship friday", "We ship Friday."),
            "We ship Friday."
        );
    }

    #[test]
    fn rejects_insertion_substitution_reorder() {
        assert!(!is_faithful("we ship friday", "We will ship Friday."));
        assert!(!is_faithful("we ship friday", "We ship Monday."));
        assert!(!is_faithful("we ship friday", "Friday we ship."));
        assert_eq!(
            choose_transcript("we ship friday", "We ship Monday."),
            "we ship friday"
        );
    }

    #[test]
    fn rejects_dropped_negation_number_and_large_deletions() {
        assert!(!is_faithful("we do not ship friday", "We do ship Friday."));
        assert!(!is_faithful("we ship 3 builds", "We ship builds."));
        assert!(!is_faithful("a b c d e f", "a"));
        assert!(!is_faithful("hello", ""));
    }
}
