//! Text conventions shared by the producer of per-sentence results (pipeline captions) and
//! their consumers (transcript), so both reassemble a segment the same way.

/// Languages written without spaces between sentences.
fn spaceless(lang: &str) -> bool {
    let primary = lang.split(['-', '_']).next().unwrap_or(lang);
    matches!(
        primary.to_ascii_lowercase().as_str(),
        "zh" | "ja" | "th" | "lo" | "km" | "my"
    )
}

/// Joins the sentences of one segment, in order: a space between them, nothing for languages
/// written without spaces. Blank parts are skipped.
pub fn join_sentences<'a>(lang: &str, parts: impl IntoIterator<Item = &'a str>) -> String {
    let sep = if spaceless(lang) { "" } else { " " };
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_by_language() {
        assert_eq!(join_sentences("en", ["Hi.", " There. ", ""]), "Hi. There.");
        assert_eq!(
            join_sentences("ja", ["こんにちは。", "元気ですか。"]),
            "こんにちは。元気ですか。"
        );
        assert_eq!(
            join_sentences("zh-CN", ["你好。", "再见。"]),
            "你好。再见。"
        );
        assert_eq!(join_sentences("vi", Vec::<&str>::new()), "");
    }
}
