//! Hallucination guard for final transcripts.
//!
//! Whisper-family models invent text when the committed audio holds little speech (a breath, a
//! click, room noise, a cut-off word): short stock replies in a language the speaker never
//! declared ("Yeah.", "How?"), or a fluent sentence far longer than the audio. Without token
//! logprobs (next-gen models do not return them) these are caught from what the pipeline knows
//! independently of the model: how long the endpointer heard speech and which language the
//! speaker declared.
//!
//! Rules (all derived, no phrase lists):
//! - **Speaking rate**: people say at most ~7 syllables per second. A transcript needing more
//!   syllables than the measured speech allows cannot have come from that audio.
//! - **Off-language filler**: a very short transcript (≤ [`FILLER_MAX_WORDS`] words) from a
//!   short turn, in a script/language other than the declared one, is the model's prior talking,
//!   not the speaker (real code-switching comes inside longer sentences).

use crate::turns::guess_language;

/// Upper bound of the speaking rate (syllables per second of detected speech).
const MAX_SYLLABLES_PER_S: f32 = 7.0;
/// Syllables allowed on top of the rate (endpointer trims onsets; one word may straddle).
const RATE_SLACK_SYLLABLES: f32 = 2.0;
/// Transcripts up to this many words count as filler for the language check.
pub const FILLER_MAX_WORDS: usize = 2;
/// Turns shorter than this (ms of speech) are where filler hallucinations happen.
const FILLER_MAX_SPEECH_MS: u32 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    TooLongForAudio,
    OffLanguageFiller,
}

/// Checks a final transcript against the turn it came from. `speech_ms == None` (unknown)
/// skips the duration-based rules.
pub fn check(text: &str, speech_ms: Option<u32>, declared: Option<&str>) -> Result<(), Rejection> {
    let Some(ms) = speech_ms else { return Ok(()) };
    let syllables = syllables(text) as f32;
    let allowed = RATE_SLACK_SYLLABLES + MAX_SYLLABLES_PER_S * ms as f32 / 1000.0;
    if syllables > allowed {
        return Err(Rejection::TooLongForAudio);
    }
    if let Some(decl) = declared.map(base).filter(|d| !d.is_empty() && d != "auto")
        && ms < FILLER_MAX_SPEECH_MS
        && words(text) <= FILLER_MAX_WORDS
    {
        let got = guess_language(text);
        if got != "und" && got != decl && !compatible(&decl, got) {
            return Err(Rejection::OffLanguageFiller);
        }
    }
    Ok(())
}

fn base(code: &str) -> String {
    code.split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// `guess_language` only tells scripts apart: Latin text without Vietnamese marks reads "en",
/// so it cannot contradict another Latin-script language that lacks distinctive letters.
fn compatible(declared: &str, guessed: &str) -> bool {
    guessed == "en" && !matches!(declared, "vi" | "ja" | "ko" | "zh" | "th" | "ru" | "ar")
}

fn words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// Syllable estimate: CJK/kana/hangul characters count one each; a Vietnamese word is one
/// syllable; other words count their vowel groups (at least one).
fn syllables(text: &str) -> usize {
    let mut total = 0;
    for w in text.split_whitespace() {
        let cjk = w.chars().filter(|c| is_cjk(*c)).count();
        if cjk > 0 {
            total += cjk;
            continue;
        }
        if !w.chars().any(char::is_alphabetic) {
            // Numbers are read aloud: count digits generously as one syllable each, capped.
            total += w.chars().filter(char::is_ascii_digit).count().min(4);
            continue;
        }
        if guess_language(w) == "vi" {
            total += 1;
            continue;
        }
        let mut groups = 0;
        let mut in_vowel = false;
        for c in w.chars().flat_map(char::to_lowercase) {
            let v = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
            if v && !in_vowel {
                groups += 1;
            }
            in_vowel = v;
        }
        // Silent final "e" ("make", "time").
        if groups > 1
            && w.to_lowercase()
                .trim_end_matches(|c: char| !c.is_alphabetic())
                .ends_with('e')
        {
            groups -= 1;
        }
        total += groups.max(1);
    }
    total
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_language_filler_in_short_turns_is_rejected() {
        assert_eq!(
            check("Yeah.", Some(500), Some("vi")),
            Err(Rejection::OffLanguageFiller)
        );
        assert_eq!(
            check("How?", Some(700), Some("vi-VN")),
            Err(Rejection::OffLanguageFiller)
        );
        // Same words from an English speaker are fine.
        assert_eq!(check("Yeah.", Some(500), Some("en")), Ok(()));
        // A Vietnamese speaker's short Vietnamese reply is fine.
        assert_eq!(check("Chưa.", Some(400), Some("vi")), Ok(()));
    }

    #[test]
    fn code_switching_inside_longer_speech_is_kept() {
        assert_eq!(check("deploy xong rồi", Some(900), Some("vi")), Ok(()));
        assert_eq!(check("OK, deploy it now", Some(2_000), Some("vi")), Ok(()));
    }

    #[test]
    fn text_longer_than_the_audio_is_rejected() {
        let invented = "Cảm ơn các bạn đã theo dõi và hẹn gặp lại các bạn trong video tiếp theo";
        assert_eq!(
            check(invented, Some(600), Some("vi")),
            Err(Rejection::TooLongForAudio)
        );
        assert_eq!(check(invented, Some(3_000), Some("vi")), Ok(()));
        assert_eq!(
            check(
                "Thank you very much for watching this video",
                Some(500),
                Some("en")
            ),
            Err(Rejection::TooLongForAudio)
        );
    }

    #[test]
    fn fast_real_speech_passes() {
        // ~5.5 syllables/s: fast but human.
        assert_eq!(
            check("cái của em nó chạy rồi nè", Some(1_100), Some("vi")),
            Ok(())
        );
        assert_eq!(
            check("Anh xem đi nhiều nhà đẹp lắm.", Some(1_200), Some("vi")),
            Ok(())
        );
    }

    #[test]
    fn unknown_duration_or_language_skips_rules() {
        assert_eq!(check("Yeah.", None, Some("vi")), Ok(()));
        assert_eq!(check("Yeah.", Some(300), None), Ok(()));
        assert_eq!(check("Yeah.", Some(300), Some("auto")), Ok(()));
    }

    #[test]
    fn syllable_estimate() {
        assert_eq!(syllables("hello world"), 3);
        assert_eq!(syllables("make time"), 2);
        assert_eq!(syllables("tiếng Việt"), 2);
        assert_eq!(syllables("会議です"), 4);
    }
}
