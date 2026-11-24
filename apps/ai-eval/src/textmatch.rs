//! Case- and diacritic-insensitive text matching (Vietnamese aware).

/// Lowercase, strip combining marks and Vietnamese diacritics, collapse whitespace.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        if ('\u{0300}'..='\u{036f}').contains(&c) {
            continue;
        }
        out.push(base_letter(c));
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn base_letter(c: char) -> char {
    const TABLE: [(char, &str); 7] = [
        ('a', "àáạảãâầấậẩẫăằắặẳẵ"),
        ('e', "èéẹẻẽêềếệểễ"),
        ('i', "ìíịỉĩ"),
        ('o', "òóọỏõôồốộổỗơờớợởỡ"),
        ('u', "ùúụủũưừứựửữ"),
        ('y', "ỳýỵỷỹ"),
        ('d', "đ"),
    ];
    for (base, variants) in TABLE {
        if variants.contains(c) {
            return base;
        }
    }
    c
}

/// True when `haystack` contains `needle` after folding.
pub fn contains(haystack: &str, needle: &str) -> bool {
    let n = fold(needle);
    !n.is_empty() && fold(haystack).contains(&n)
}

/// A `must_include` item: `a|b` means any alternative is enough.
pub fn contains_any_alt(haystack: &str, item: &str) -> bool {
    let h = fold(haystack);
    item.split('|').any(|alt| {
        let a = fold(alt);
        !a.is_empty() && h.contains(&a)
    })
}

/// Every item satisfied (empty list: satisfied).
pub fn contains_all(haystack: &str, items: &[String]) -> bool {
    items.iter().all(|i| contains_any_alt(haystack, i))
}

/// First forbidden phrase present, if any.
pub fn first_hit<'a>(haystack: &str, phrases: &'a [String]) -> Option<&'a str> {
    let h = fold(haystack);
    phrases.iter().map(String::as_str).find(|p| {
        let f = fold(p);
        !f.is_empty() && h.contains(&f)
    })
}

/// Keyword match used for decisions and action items: at least half of the keywords (rounded
/// up, minimum one) occur in `text`.
pub fn keywords_match(text: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return false;
    }
    let h = fold(text);
    let found = keywords
        .iter()
        .filter(|k| {
            let f = fold(k);
            !f.is_empty() && h.contains(&f)
        })
        .count();
    found >= keywords.len().div_ceil(2).max(1)
}

/// Fraction of keywords found, for ranking candidates.
pub fn keyword_score(text: &str, keywords: &[String]) -> f64 {
    if keywords.is_empty() {
        return 0.0;
    }
    let h = fold(text);
    let found = keywords
        .iter()
        .filter(|k| {
            let f = fold(k);
            !f.is_empty() && h.contains(&f)
        })
        .count();
    found as f64 / keywords.len() as f64
}

/// Content words of `s` (folded, at least 3 chars), used by the toy retriever.
pub fn words(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn folds_vietnamese() {
        assert_eq!(fold("Thứ Sáu, ĐÃ chốt"), "thu sau, da chot");
        assert!(contains("Giảm GIÁ 30%", "giam gia 30%"));
    }

    #[test]
    fn alternatives_and_threshold() {
        assert!(contains_any_alt("còn 12 ngày", "mười hai|12"));
        assert!(!contains_any_alt("còn 8 ngày", "mười hai|12"));
        let kw = vec!["a".to_string(), "b".into(), "c".into()];
        assert!(keywords_match("a b", &kw));
        assert!(!keywords_match("a", &kw));
        assert!(!keywords_match("anything", &[]));
    }
}
