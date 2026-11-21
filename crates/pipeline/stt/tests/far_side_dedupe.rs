#![allow(clippy::unwrap_used)]

//! Port of upstream `warptalk-ai/tests/test_far_side_dedupe.py` (forward direction), plus
//! Vietnamese, Japanese and bounded-memory cases.

use warptalk_stt::far_side_dedupe::{
    DedupeReason, FarSideDedupe, FarSideDedupeConfig, FarSideDedupeConfigError,
};

const LINE: &str = "we should ship the bridge build on friday";
const T0: u64 = 1_759_300_000_000;

fn with_config(cfg: FarSideDedupeConfig, named: &[(&str, &str, u64)]) -> FarSideDedupe {
    let mut d = FarSideDedupe::new(cfg);
    for (text, lang, end) in named {
        d.observe_participant(text, lang, *end);
    }
    d
}

/// Stand-in segment whose audio ends `lag_ms` after the named line (which ends at T0).
fn standin(d: &FarSideDedupe, text: &str, lag_ms: i64, language: &str) -> bool {
    let end = (T0 as i64 + lag_ms) as u64;
    d.should_drop_far_side(text, language, end - 1_000, end)
        .drop
}

fn forward(cfg: FarSideDedupeConfig, text: &str, lag_ms: i64) -> bool {
    let d = with_config(cfg, &[(LINE, "en", T0)]);
    standin(&d, text, lag_ms, "en")
}

fn default_forward(text: &str, lag_ms: i64) -> bool {
    forward(FarSideDedupeConfig::default(), text, lag_ms)
}

#[test]
fn stand_in_after_named_is_dropped_within_lag_band() {
    for lag in [150, 300, 600, 900, 2_000] {
        assert!(default_forward(LINE, lag), "lag {lag} should drop");
    }
}

#[test]
fn stand_in_first_or_simultaneous_is_kept() {
    // The leak case: the stand-in carries the true Meet-side speech.
    for lag in [-1_000, -300, 0, 100, 149] {
        assert!(!default_forward(LINE, lag), "lag {lag} should keep");
    }
}

#[test]
fn beyond_max_lag_is_kept_and_configurable() {
    assert!(!default_forward(LINE, 2_001));
    let cfg = FarSideDedupeConfig {
        max_lag_ms: 3_000,
        ..Default::default()
    };
    assert!(forward(cfg, LINE, 2_500));
}

#[test]
fn min_lag_is_configurable() {
    let cfg = FarSideDedupeConfig {
        min_lag_ms: 50,
        ..Default::default()
    };
    assert!(forward(cfg, LINE, 100));
}

#[test]
fn missing_timing_keeps_both() {
    let d = with_config(FarSideDedupeConfig::default(), &[(LINE, "en", T0)]);
    // start_ms 0 is what the room's first chunk is clamped to: unknown.
    let v = d.should_drop_far_side(LINE, "en", 0, T0 + 400);
    assert!(!v.drop);
    assert_eq!(v.reason, DedupeReason::TimingUnknown);
    // start after end is also unusable.
    assert!(!d.should_drop_far_side(LINE, "en", T0 + 500, T0 + 400).drop);
}

#[test]
fn fragment_of_the_named_line_matches() {
    assert!(default_forward("ship the bridge build", 400));
}

#[test]
fn small_mishearing_matches_at_default_ratio() {
    assert!(default_forward(
        "we should ship the bridge built on friday",
        400
    ));
}

#[test]
fn ratio_threshold_is_respected() {
    let heard = "we could skip the fridge build on monday";
    let low = FarSideDedupeConfig {
        min_ratio: 0.6,
        ..Default::default()
    };
    let high = FarSideDedupeConfig {
        min_ratio: 0.95,
        ..Default::default()
    };
    assert!(forward(low, heard, 400));
    assert!(!forward(high, heard, 400));
}

#[test]
fn unrelated_line_does_not_match() {
    assert!(!default_forward(
        "can everyone see my screen right now",
        400
    ));
}

#[test]
fn short_back_channel_is_never_dropped() {
    let d = with_config(FarSideDedupeConfig::default(), &[("okay", "en", T0)]);
    let v = d.should_drop_far_side("okay", "en", T0 + 300, T0 + 400);
    assert!(!v.drop);
    assert_eq!(v.reason, DedupeReason::TooShort);
}

#[test]
fn min_chars_is_configurable() {
    let cfg = FarSideDedupeConfig {
        min_chars: 3,
        ..Default::default()
    };
    let d = with_config(cfg, &[("okay", "en", T0)]);
    assert!(
        d.should_drop_far_side("okay", "en", T0 + 300, T0 + 400)
            .drop
    );
}

#[test]
fn outside_the_timestamp_window_does_not_match() {
    let cfg = FarSideDedupeConfig {
        window_ms: 5_000,
        max_lag_ms: 10_000,
        ..Default::default()
    };
    let d = with_config(cfg, &[(LINE, "en", T0)]);
    assert!(
        !d.should_drop_far_side(LINE, "en", T0 + 5_700, T0 + 6_000)
            .drop
    );
}

#[test]
fn different_language_is_kept_unless_same_language_is_off() {
    let d = with_config(FarSideDedupeConfig::default(), &[(LINE, "en", T0)]);
    assert!(!standin(&d, LINE, 400, "vi"));
    let cfg = FarSideDedupeConfig {
        same_language: false,
        ..Default::default()
    };
    assert!(forward(cfg, LINE, 400));
}

#[test]
fn language_tags_are_normalized() {
    let d = with_config(FarSideDedupeConfig::default(), &[(LINE, "en-US", T0)]);
    assert!(standin(&d, LINE, 400, "en"));
    assert!(standin(&d, LINE, 400, "auto"));
    assert!(standin(&d, LINE, 400, ""));
    let d = with_config(FarSideDedupeConfig::default(), &[(LINE, "vi", T0)]);
    assert!(!standin(&d, LINE, 400, "en"));
}

#[test]
fn longer_stand_in_line_containing_the_named_one_is_kept() {
    // The rest of this line is somebody else's words.
    assert!(!default_forward(
        &format!("{LINE} and also we need to book the demo room for the client"),
        400
    ));
}

#[test]
fn best_match_among_several_participant_lines_drops() {
    let d = with_config(
        FarSideDedupeConfig::default(),
        &[
            ("we should ship the bridge build on monday", "en", T0),
            (LINE, "en", T0 + 50),
        ],
    );
    let v = d.should_drop_far_side(
        "we should ship the bridge build on fridays",
        "en",
        T0 + 300,
        T0 + 450,
    );
    assert!(v.drop);
    assert_eq!(v.reason, DedupeReason::FarSideEcho);
}

#[test]
fn matched_ratio_is_difflib_ratio() {
    // Python: difflib.SequenceMatcher(None, "we should ship the bridge built on friday",
    //         "we should ship the bridge build on friday").ratio() = 0.975609...
    let d = with_config(FarSideDedupeConfig::default(), &[(LINE, "en", T0)]);
    let v = d.should_drop_far_side(
        "we should ship the bridge built on friday",
        "en",
        T0 + 300,
        T0 + 400,
    );
    assert!(v.drop);
    assert!(
        (v.matched_ratio - 0.975_61).abs() < 1e-4,
        "got {}",
        v.matched_ratio
    );
}

#[test]
fn disabled_keeps_the_copy() {
    let cfg = FarSideDedupeConfig {
        enabled: false,
        ..Default::default()
    };
    let mut d = FarSideDedupe::new(cfg);
    d.observe_participant(LINE, "en", T0);
    assert_eq!(d.observed_len(), 0);
    let v = d.should_drop_far_side(LINE, "en", T0 + 300, T0 + 400);
    assert!(!v.drop);
    assert_eq!(v.reason, DedupeReason::Disabled);
}

#[test]
fn vietnamese_diacritics_match() {
    let line = "Chúng ta nên đẩy bản dựng cầu vào thứ sáu nhé";
    let d = with_config(FarSideDedupeConfig::default(), &[(line, "vi", T0)]);
    // Same sentence with the tone marks in decomposed (NFD) form.
    let decomposed = "Chu\u{0301}ng ta n\u{00ea}n \u{0111}\u{1ea9}y ba\u{0309}n d\u{01b0}\u{0301}ng c\u{00e2}\u{0300}u v\u{00e0}o th\u{01b0}\u{0301} s\u{00e1}u nh\u{00e9}";
    assert!(standin(&d, decomposed, 400, "vi"));
    // Small mishearing that keeps the diacritics still matches.
    assert!(standin(
        &d,
        "Chúng ta nên đẩy bản dựng cầu vào thứ sáu nhỉ",
        400,
        "vi"
    ));
    // Unrelated Vietnamese sentence is kept.
    assert!(!standin(
        &d,
        "Tôi không chắc về ngân sách của quý này",
        400,
        "vi"
    ));
}

#[test]
fn vietnamese_diacritic_loss_is_a_weaker_match() {
    // Diacritic-stripped copy: a different string, but similar enough to be an echo of the line.
    let line = "Chúng ta nên đẩy bản dựng cầu vào thứ sáu nhé";
    let d = with_config(FarSideDedupeConfig::default(), &[(line, "vi", T0)]);
    let v = d.should_drop_far_side(
        "chung ta nen day ban dung cau vao thu sau nhe",
        "vi",
        T0 + 300,
        T0 + 400,
    );
    assert!(v.matched_ratio < 1.0);
    assert!(v.matched_ratio > 0.0);
}

#[test]
fn japanese_without_spaces_matches() {
    let line = "来週の金曜日にブリッジのビルドをリリースしましょう。";
    let d = with_config(FarSideDedupeConfig::default(), &[(line, "ja", T0)]);
    assert!(standin(
        &d,
        "来週の金曜日にブリッジのビルドをリリースしましょう",
        400,
        "ja"
    ));
    assert!(standin(
        &d,
        "来週の金曜日にブリッジのビルドをリリースしましょう！",
        400,
        "ja"
    ));
    assert!(!standin(
        &d,
        "予算の見直しは来月に延期しませんか",
        400,
        "ja"
    ));
}

#[test]
fn participant_buffer_is_bounded_and_evicts_outside_window() {
    let cfg = FarSideDedupeConfig {
        window_ms: 10_000,
        ..Default::default()
    };
    let mut d = FarSideDedupe::new(cfg);
    for i in 0..1_000_u64 {
        d.observe_participant(
            &format!("line number {i} about the bridge build"),
            "en",
            T0 + i,
        );
    }
    assert!(
        d.observed_len() <= 64,
        "buffer grew to {}",
        d.observed_len()
    );

    // A line older than the window is evicted by a newer observation.
    let mut d = FarSideDedupe::new(FarSideDedupeConfig {
        window_ms: 10_000,
        ..Default::default()
    });
    d.observe_participant(LINE, "en", T0);
    d.observe_participant("something completely different happened", "en", T0 + 20_000);
    assert_eq!(d.observed_len(), 1);
    assert!(!standin(&d, LINE, 400, "en"));
}

#[test]
fn empty_and_punctuation_only_lines_are_not_remembered() {
    let mut d = FarSideDedupe::new(FarSideDedupeConfig::default());
    d.observe_participant("", "en", T0);
    d.observe_participant(" ... !!! ", "en", T0);
    assert_eq!(d.observed_len(), 0);
}

#[test]
fn config_defaults_match_upstream() {
    let cfg: FarSideDedupeConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg, FarSideDedupeConfig::default());
    assert!(cfg.enabled);
    assert_eq!(cfg.window_ms, 15_000);
    assert_eq!(cfg.min_lag_ms, 150);
    assert_eq!(cfg.max_lag_ms, 2_000);
    assert!((cfg.min_ratio - 0.8).abs() < f32::EPSILON);
    assert_eq!(cfg.min_chars, 8);
    assert!(cfg.same_language);
}

#[test]
fn config_deserializes_partial_overrides() {
    let cfg: FarSideDedupeConfig =
        serde_json::from_str(r#"{"enabled": false, "min_ratio": 0.9}"#).unwrap();
    assert!(!cfg.enabled);
    assert!((cfg.min_ratio - 0.9).abs() < f32::EPSILON);
    assert_eq!(cfg.window_ms, 15_000);
}

#[test]
fn config_validation() {
    assert!(FarSideDedupeConfig::default().validate().is_ok());
    let zero_window = FarSideDedupeConfig {
        window_ms: 0,
        ..Default::default()
    };
    assert_eq!(
        zero_window.validate(),
        Err(FarSideDedupeConfigError::ZeroWindow)
    );
    let bad_ratio = FarSideDedupeConfig {
        min_ratio: 1.5,
        ..Default::default()
    };
    assert_eq!(
        bad_ratio.validate(),
        Err(FarSideDedupeConfigError::RatioOutOfRange)
    );
    let nan_ratio = FarSideDedupeConfig {
        min_ratio: f32::NAN,
        ..Default::default()
    };
    assert_eq!(
        nan_ratio.validate(),
        Err(FarSideDedupeConfigError::RatioOutOfRange)
    );
    let zero_chars = FarSideDedupeConfig {
        min_chars: 0,
        ..Default::default()
    };
    assert_eq!(
        zero_chars.validate(),
        Err(FarSideDedupeConfigError::ZeroMinChars)
    );
    let inverted = FarSideDedupeConfig {
        min_lag_ms: 3_000,
        max_lag_ms: 2_000,
        ..Default::default()
    };
    assert_eq!(
        inverted.validate(),
        Err(FarSideDedupeConfigError::LagInverted {
            min: 3_000,
            max: 2_000
        })
    );
}
