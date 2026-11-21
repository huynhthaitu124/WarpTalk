//! Language codes and names. Source and target codes are normalised to the base subtag
//! (`vi-VN` -> `vi`); equal languages skip translation (spec 13 section 2.4).

pub fn base_language(lang: &str) -> String {
    lang.split(['-', '_'])
        .next()
        .unwrap_or(lang)
        .trim()
        .to_ascii_lowercase()
}

pub fn is_same_language(a: &str, b: &str) -> bool {
    let (a, b) = (base_language(a), base_language(b));
    !a.is_empty() && a == b
}

/// English name used in prompts (the model is prompted with the name, so any pair in the table
/// works: no per-pair allow-list). Unknown codes are passed through as the code itself.
pub fn language_name(lang: &str) -> String {
    let code = base_language(lang);
    let name = match code.as_str() {
        "en" => "English",
        "vi" => "Vietnamese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" => "Chinese",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ru" => "Russian",
        "th" => "Thai",
        "id" => "Indonesian",
        "ms" => "Malay",
        "hi" => "Hindi",
        "ar" => "Arabic",
        "tr" => "Turkish",
        "nl" => "Dutch",
        "pl" => "Polish",
        "sv" => "Swedish",
        "uk" => "Ukrainian",
        _ => return code,
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_and_compares() {
        assert_eq!(base_language("vi-VN"), "vi");
        assert_eq!(base_language("PT_br"), "pt");
        assert!(is_same_language("vi", "vi-VN"));
        assert!(!is_same_language("vi", "en"));
        assert!(!is_same_language("", ""));
        assert_eq!(language_name("ja-JP"), "Japanese");
        assert_eq!(language_name("xx"), "xx");
    }
}
