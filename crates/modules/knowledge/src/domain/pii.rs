//! Local PII / DLP heuristics (no external service, no regex dependency): emails, phone numbers
//! (vi / ja / us / international), payment card numbers (Luhn) and national ids (vi CCCD, ja My
//! Number check digit, us SSN, plus keyword-anchored 9/12 digit ids).
//!
//! Detection favours precision: structural checks (Luhn, check digits, province codes) gate the
//! digit-run detectors so that ordinary numbers are left alone.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    Email,
    Phone,
    Card,
    NationalId,
}

impl PiiKind {
    pub fn mask_token(self) -> &'static str {
        match self {
            Self::Email => "[EMAIL]",
            Self::Phone => "[PHONE]",
            Self::Card => "[CARD]",
            Self::NationalId => "[ID]",
        }
    }
}

/// Byte range in the scanned text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiiMatch {
    pub kind: PiiKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiPolicy {
    Off,
    /// Replace each hit with a token (`[EMAIL]`) before the text is chunked and embedded.
    #[default]
    Mask,
    /// Drop any text unit (segment, page, chunk) that contains a hit.
    Block,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PiiOutcome {
    /// `None` when blocked.
    pub text: Option<String>,
    pub hits: Vec<PiiKind>,
}

impl PiiOutcome {
    pub fn blocked(&self) -> bool {
        self.text.is_none()
    }
}

/// Apply `policy` to one text unit.
pub fn apply_policy(text: &str, policy: PiiPolicy) -> PiiOutcome {
    if policy == PiiPolicy::Off {
        return PiiOutcome {
            text: Some(text.to_owned()),
            hits: Vec::new(),
        };
    }
    let matches = detect(text);
    let hits: Vec<PiiKind> = matches.iter().map(|m| m.kind).collect();
    if matches.is_empty() {
        return PiiOutcome {
            text: Some(text.to_owned()),
            hits,
        };
    }
    match policy {
        PiiPolicy::Block => PiiOutcome { text: None, hits },
        _ => PiiOutcome {
            text: Some(mask(text, &matches)),
            hits,
        },
    }
}

pub fn mask(text: &str, matches: &[PiiMatch]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for m in matches {
        if m.start < pos {
            continue;
        }
        out.push_str(&text[pos..m.start]);
        out.push_str(m.kind.mask_token());
        pos = m.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// All hits, sorted by position and non-overlapping.
pub fn detect(text: &str) -> Vec<PiiMatch> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = detect_emails(text, &chars);
    detect_digit_runs(text, &chars, &mut found);
    found.sort_by_key(|m| m.start);
    found
}

fn byte_at(text: &str, chars: &[(usize, char)], i: usize) -> usize {
    chars.get(i).map_or(text.len(), |(b, _)| *b)
}

fn detect_emails(text: &str, chars: &[(usize, char)]) -> Vec<PiiMatch> {
    let local_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let domain_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].1 != '@' {
            i += 1;
            continue;
        }
        let mut s = i;
        while s > 0 && local_ok(chars[s - 1].1) {
            s -= 1;
        }
        let mut e = i + 1;
        while e < chars.len() && domain_ok(chars[e].1) {
            e += 1;
        }
        // trim trailing punctuation from the domain
        while e > i + 1 && matches!(chars[e - 1].1, '.' | '-') {
            e -= 1;
        }
        let domain: String = chars[i + 1..e].iter().map(|(_, c)| *c).collect();
        let tld_ok = domain
            .rsplit_once('.')
            .is_some_and(|(_, t)| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()));
        if s < i && tld_ok {
            out.push(PiiMatch {
                kind: PiiKind::Email,
                start: byte_at(text, chars, s),
                end: byte_at(text, chars, e),
            });
            i = e;
        } else {
            i += 1;
        }
    }
    out
}

fn luhn_ok(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = u32::from(*d);
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum % 10 == 0
}

/// Japanese My Number: 12 digits, the last is a check digit over the first eleven.
fn my_number_ok(d: &[u8]) -> bool {
    if d.len() != 12 {
        return false;
    }
    let mut sum = 0u32;
    for n in 1..=11usize {
        let p = u32::from(d[11 - n]);
        let q = if n <= 6 { n as u32 + 1 } else { n as u32 - 5 };
        sum += p * q;
    }
    let rem = sum % 11;
    let check = if rem <= 1 { 0 } else { 11 - rem };
    check == u32::from(d[11])
}

const VN_PROVINCES: &[u16] = &[
    1, 2, 4, 6, 8, 10, 11, 12, 14, 15, 17, 19, 20, 22, 24, 25, 26, 27, 30, 31, 33, 34, 35, 36, 37,
    38, 40, 42, 44, 45, 46, 48, 49, 51, 52, 54, 56, 58, 60, 62, 64, 66, 67, 68, 70, 72, 74, 75, 77,
    79, 80, 82, 83, 84, 86, 87, 89, 91, 92, 93, 94, 95, 96,
];

/// Vietnamese citizen id (CCCD): 3-digit province code, century/gender digit, 2-digit year.
fn vn_cccd_ok(d: &[u8]) -> bool {
    if d.len() != 12 {
        return false;
    }
    let province = u16::from(d[0]) * 100 + u16::from(d[1]) * 10 + u16::from(d[2]);
    VN_PROVINCES.contains(&province)
}

fn us_ssn_ok(d: &[u8]) -> bool {
    if d.len() != 9 {
        return false;
    }
    let area = u16::from(d[0]) * 100 + u16::from(d[1]) * 10 + u16::from(d[2]);
    let group = d[3] * 10 + d[4];
    let serial = u16::from(d[5]) * 1000 + u16::from(d[6]) * 100 + u16::from(d[7]) * 10 + u16::from(d[8]);
    area != 0 && area != 666 && area < 900 && group != 0 && serial != 0
}

const ID_KEYWORDS: &[&str] = &[
    "cmnd",
    "cmt",
    "cccd",
    "căn cước",
    "can cuoc",
    "chứng minh",
    "chung minh",
    "my number",
    "mynumber",
    "マイナンバー",
    "個人番号",
    "身分証",
    "ssn",
    "social security",
    "national id",
    "id number",
    "passport",
    "hộ chiếu",
    "ho chieu",
];

fn keyword_before(chars: &[(usize, char)], start: usize) -> bool {
    let from = start.saturating_sub(40);
    let window: String = chars[from..start].iter().map(|(_, c)| *c).collect();
    let window = window.to_lowercase();
    ID_KEYWORDS.iter().any(|k| window.contains(k))
}

struct Run {
    /// char index range [start, end)
    start: usize,
    end: usize,
    digits: Vec<u8>,
    plus: bool,
    /// digit group sizes split by separators
    groups: Vec<usize>,
    seps: Vec<char>,
    paren: bool,
}

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '-' | '.' | '(' | ')')
}

fn scan_run(chars: &[(usize, char)], start: usize) -> Option<Run> {
    let first = chars[start].1;
    let (mut i, plus) = match first {
        '+' => (start + 1, true),
        '(' => (start + 1, false),
        c if c.is_ascii_digit() => (start, false),
        _ => return None,
    };
    if !chars.get(i).is_some_and(|(_, c)| c.is_ascii_digit()) {
        return None;
    }
    let mut digits = Vec::new();
    let mut groups = Vec::new();
    let mut seps = Vec::new();
    let mut cur = 0usize;
    let mut paren = first == '(';
    let mut last_digit_end = i;
    while i < chars.len() {
        let c = chars[i].1;
        if c.is_ascii_digit() {
            digits.push(c as u8 - b'0');
            cur += 1;
            i += 1;
            last_digit_end = i;
        } else if is_sep(c) && cur > 0 {
            // single separators between digits; allow ") " and " (" combos
            let mut j = i;
            let mut sep_chars = Vec::new();
            while j < chars.len() && is_sep(chars[j].1) && sep_chars.len() < 2 {
                sep_chars.push(chars[j].1);
                j += 1;
            }
            if !chars.get(j).is_some_and(|(_, c)| c.is_ascii_digit()) {
                // trailing separator: a closing paren belongs to the run
                if sep_chars.first() == Some(&')') && paren {
                    last_digit_end = i + 1;
                }
                break;
            }
            let allowed_pair = sep_chars.len() == 1
                || (sep_chars.len() == 2
                    && ((sep_chars[0] == ')' && sep_chars[1] == ' ')
                        || (sep_chars[0] == ' ' && sep_chars[1] == '(')
                        || (sep_chars[0] == '-' && sep_chars[1] == '(')));
            if !allowed_pair {
                break;
            }
            if sep_chars.contains(&'(') || sep_chars.contains(&')') {
                paren = true;
            }
            groups.push(cur);
            cur = 0;
            seps.push(sep_chars[0]);
            i = j;
        } else {
            break;
        }
    }
    if cur > 0 {
        groups.push(cur);
    }
    Some(Run {
        start,
        end: last_digit_end,
        digits,
        plus,
        groups,
        seps,
        paren,
    })
}

fn classify(run: &Run, chars: &[(usize, char)]) -> Option<PiiKind> {
    let n = run.digits.len();
    let uniform_seps = run.seps.windows(2).all(|w| w[0] == w[1]);
    let no_seps = run.seps.is_empty();

    // Payment card.
    if (13..=19).contains(&n)
        && !run.plus
        && !run.paren
        && matches!(run.digits[0], 2..=6)
        && (no_seps
            || (uniform_seps
                && matches!(run.seps[0], ' ' | '-')
                && run.groups[..run.groups.len() - 1].iter().all(|g| *g >= 4)))
        && run.digits.iter().any(|d| *d != run.digits[0])
        && luhn_ok(&run.digits)
    {
        return Some(PiiKind::Card);
    }

    // National ids.
    if n == 12 && !run.plus && !run.paren {
        let grouped_4 = run.groups == [4, 4, 4] && uniform_seps && run.seps[0] != '.';
        if no_seps || grouped_4 {
            if vn_cccd_ok(&run.digits) || my_number_ok(&run.digits) {
                return Some(PiiKind::NationalId);
            }
            if no_seps && keyword_before(chars, run.start) {
                return Some(PiiKind::NationalId);
            }
        }
    }
    if n == 9 && !run.plus && !run.paren {
        if run.groups == [3, 2, 4] && run.seps.iter().all(|s| *s == '-') && us_ssn_ok(&run.digits) {
            return Some(PiiKind::NationalId);
        }
        if no_seps && keyword_before(chars, run.start) {
            return Some(PiiKind::NationalId);
        }
    }

    // Phone numbers.
    if (9..=15).contains(&n) {
        let international = run.plus && n >= 10;
        let trunk_zero = run.digits[0] == 0 && (10..=11).contains(&n) && !run.plus;
        // (415) 555-2671, 415-555-2671, 415.555.2671
        let us_style = n == 10 && !run.plus && run.groups == [3, 3, 4];
        if international || trunk_zero || us_style {
            return Some(PiiKind::Phone);
        }
    }
    None
}

fn detect_digit_runs(text: &str, chars: &[(usize, char)], found: &mut Vec<PiiMatch>) {
    let email_ranges: Vec<(usize, usize)> = found.iter().map(|m| (m.start, m.end)).collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        let starts = c.is_ascii_digit() || c == '+' || c == '(';
        let boundary_ok = i == 0 || !chars[i - 1].1.is_alphanumeric() || is_cjk_char(chars[i - 1].1);
        if !starts || !boundary_ok {
            i += 1;
            continue;
        }
        let Some(run) = scan_run(chars, i) else {
            i += 1;
            continue;
        };
        let end_ok = chars
            .get(run.end)
            .is_none_or(|(_, c)| !c.is_alphanumeric() || is_cjk_char(*c));
        let b_start = chars[run.start].0;
        let b_end = byte_at(text, chars, run.end);
        let in_email = email_ranges.iter().any(|(s, e)| b_start < *e && b_end > *s);
        if end_ok
            && !in_email
            && let Some(kind) = classify(&run, chars)
        {
            found.push(PiiMatch {
                kind,
                start: b_start,
                end: b_end,
            });
            i = run.end.max(i + 1);
        } else {
            // skip past this run so "1 2 3" style sequences are not rescanned quadratically
            i = run.end.max(i + 1);
        }
    }
}

fn is_cjk_char(c: char) -> bool {
    super::text::is_cjk(c)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn kinds(t: &str) -> Vec<PiiKind> {
        detect(t).into_iter().map(|m| m.kind).collect()
    }

    #[test]
    fn emails() {
        assert_eq!(kinds("mail me at john.doe+x@example.co.uk, thanks"), [PiiKind::Email]);
        assert!(kinds("handle @team and a@b").is_empty());
        let out = apply_policy("write to a.b@c.io now", PiiPolicy::Mask);
        assert_eq!(out.text.unwrap(), "write to [EMAIL] now");
    }

    #[test]
    fn cards_need_luhn() {
        assert_eq!(kinds("card 4111 1111 1111 1111 ok"), [PiiKind::Card]);
        assert_eq!(kinds("card 4111-1111-1111-1111."), [PiiKind::Card]);
        assert_eq!(kinds("4242424242424242"), [PiiKind::Card]);
        assert!(kinds("order 4111 1111 1111 1112").is_empty());
        assert!(kinds("id 1234567890123456").is_empty());
    }

    #[test]
    fn phones_vi_ja_us_intl() {
        assert_eq!(kinds("gọi 0912 345 678 nhé"), [PiiKind::Phone]);
        assert_eq!(kinds("電話は090-1234-5678です"), [PiiKind::Phone]);
        assert_eq!(kinds("+81 90 1234 5678"), [PiiKind::Phone]);
        assert_eq!(kinds("+84 912 345 678"), [PiiKind::Phone]);
        assert_eq!(kinds("call (415) 555-2671 today"), [PiiKind::Phone]);
        assert_eq!(kinds("call 415-555-2671"), [PiiKind::Phone]);
        assert!(kinds("we sold 2024 units on 2024-01-15 at 10:30").is_empty());
        assert!(kinds("version 1.2.3.4 build 123456").is_empty());
        assert!(kinds("budget 1,234,567 dollars").is_empty());
    }

    #[test]
    fn national_ids() {
        // vi CCCD: Hanoi (001) prefix
        assert_eq!(kinds("CCCD 001204012345 của tôi"), [PiiKind::NationalId]);
        // province 000 / 097 is not valid and there is no keyword
        assert!(kinds("ref 097204012345").is_empty());
        // ja My Number with a valid check digit (computed from the algorithm)
        let mut d = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 0];
        let mut sum = 0u32;
        for n in 1..=11usize {
            let q = if n <= 6 { n as u32 + 1 } else { n as u32 - 5 };
            sum += u32::from(d[11 - n]) * q;
        }
        let rem = sum % 11;
        d[11] = if rem <= 1 { 0 } else { (11 - rem) as u8 };
        let s: String = d.iter().map(|x| char::from(b'0' + x)).collect();
        assert!(my_number_ok(&d));
        assert_eq!(kinds(&format!("マイナンバー {s}")), [PiiKind::NationalId]);
        // us ssn
        assert_eq!(kinds("SSN 123-45-6789"), [PiiKind::NationalId]);
        assert!(kinds("000-12-3456 and 666-12-3456 and 900-12-3456").is_empty());
        // 9-digit id only with a keyword
        assert_eq!(kinds("CMND: 012345678"), [PiiKind::NationalId]);
        assert!(kinds("room 012345678").is_empty());
    }

    #[test]
    fn mask_and_block_policies() {
        let t = "Email a@b.com or call 0912345678.";
        let m = apply_policy(t, PiiPolicy::Mask);
        assert_eq!(m.text.as_deref(), Some("Email [EMAIL] or call [PHONE]."));
        assert_eq!(m.hits, [PiiKind::Email, PiiKind::Phone]);
        let b = apply_policy(t, PiiPolicy::Block);
        assert!(b.blocked());
        let off = apply_policy(t, PiiPolicy::Off);
        assert_eq!(off.text.as_deref(), Some(t));
        let clean = apply_policy("nothing here", PiiPolicy::Block);
        assert_eq!(clean.text.as_deref(), Some("nothing here"));
    }

    #[test]
    fn digits_inside_email_are_not_double_counted() {
        assert_eq!(kinds("user0912345678@example.com"), [PiiKind::Email]);
    }
}
