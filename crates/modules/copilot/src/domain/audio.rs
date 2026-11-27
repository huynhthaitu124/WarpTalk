//! Wire protocol and pure rules of the server-side STT fallback WebSocket
//! (`GET /copilot/sessions/{id}/audio`).
//!
//! # Framing (one socket carries both channels)
//!
//! * **Binary frame, client -> server**: byte 0 = channel (`0` me, `1` others, `2..=4` the
//!   meeting bot's tracks `bot0..bot2`), the rest is
//!   PCM16 little-endian, 16 kHz, mono (even length, at most `max_frame_bytes`). Only speech
//!   regions are sent (copilot-core's endpointer already gates).
//! * **Text frames** (JSON, `type` tag): client -> server [`ClientMsg`], server -> client
//!   [`ServerMsg`]. An utterance is the audio frames of one channel followed by `commit`
//!   (carrying its position on the session timeline).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{BOT_CHANNELS, Channel, LanguageMode};

pub const SAMPLE_RATE_HZ: u32 = 16_000;

impl Serialize for Channel {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Channel::parse(&s)
            .ok_or_else(|| serde::de::Error::custom("channel must be me, others or bot0..bot2"))
    }
}

impl Channel {
    /// Tag byte of a binary frame.
    pub fn tag(&self) -> u8 {
        match self {
            Self::Me => 0,
            Self::Others => 1,
            Self::Bot(n) => 2 + (*n).min(BOT_CHANNELS - 1),
        }
    }

    pub fn from_tag(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Me),
            1 => Some(Self::Others),
            n if (2..2 + BOT_CHANNELS).contains(&n) => Some(Self::Bot(n - 2)),
            _ => None,
        }
    }

    pub fn index(&self) -> usize {
        self.tag() as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("empty frame")]
    Empty,
    #[error("unknown channel tag {0}")]
    UnknownChannel(u8),
    #[error("pcm payload must be a whole number of 16-bit samples")]
    OddLength,
    #[error("frame larger than {0} bytes")]
    TooLarge(usize),
}

/// Parse a binary audio frame into its channel and PCM16 samples.
pub fn parse_audio_frame(
    bytes: &[u8],
    max_bytes: usize,
) -> Result<(Channel, Vec<i16>), FrameError> {
    let (&tag, payload) = bytes.split_first().ok_or(FrameError::Empty)?;
    let channel = Channel::from_tag(tag).ok_or(FrameError::UnknownChannel(tag))?;
    if bytes.len() > max_bytes {
        return Err(FrameError::TooLarge(max_bytes));
    }
    if payload.is_empty() {
        return Err(FrameError::Empty);
    }
    if payload.len() % 2 != 0 {
        return Err(FrameError::OddLength);
    }
    Ok((
        channel,
        payload
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect(),
    ))
}

/// Encode a frame (used by tests and as the reference for clients).
pub fn encode_audio_frame(channel: Channel, pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + pcm.len() * 2);
    out.push(channel.tag());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Client -> server control messages.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ClientMsg {
    /// Optional, before the first audio. `run_id` makes server-generated segment ids stable
    /// across reconnects of the same client run (resending a commit is then idempotent).
    Hello {
        #[serde(default)]
        run_id: Option<String>,
        #[serde(default)]
        language_mode: Option<String>,
        #[serde(default)]
        languages: Vec<String>,
        #[serde(default)]
        vocabulary: Vec<String>,
    },
    /// End of an utterance on `channel`. `seq` is the client's per-channel utterance counter;
    /// `start_ms`/`end_ms` are its position on the session timeline.
    Commit {
        channel: Channel,
        seq: u64,
        start_ms: i64,
        end_ms: i64,
        /// Who spoke this utterance (meeting bot: the caption name); far-side channels only.
        #[serde(default)]
        speaker: Option<String>,
    },
    /// Throw away the audio sent since the last commit on `channel`.
    Discard {
        channel: Channel,
    },
    /// Finish the provider session of `channel` (the client rotates its sessions).
    CloseChannel {
        channel: Channel,
    },
    Ping,
}

/// Server -> client messages.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ServerMsg {
    Ready {
        connection_id: String,
        sample_rate: u32,
        max_frame_bytes: usize,
        max_session_secs: u64,
    },
    /// Accumulated text of the utterance in progress; empty text clears it.
    Partial {
        channel: Channel,
        text: String,
    },
    /// A segment was written to the transcript (or was a duplicate of one that already was).
    Segment {
        channel: Channel,
        client_segment_id: String,
        segment_id: Uuid,
        text: String,
        language: String,
        start_ms: i64,
        end_ms: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
        /// Not final until the utterance's last segment; early sentences are `false`.
        early: bool,
        duplicate: bool,
        insufficient_credits: bool,
    },
    UtteranceDone {
        channel: Channel,
        seq: u64,
        ok: bool,
    },
    /// `fatal`: the server closes the socket right after.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        channel: Option<Channel>,
        fatal: bool,
    },
    Closing {
        reason: String,
    },
    Pong,
}

/// Deterministic id of a server-produced segment: stable under resends of the same commit.
pub fn server_segment_id(run: &str, channel: Channel, seq: u64, n: u32) -> String {
    let id = format!("srv-{run}-{}-{seq}-{n}", channel.as_str());
    id.chars()
        .take(super::MAX_CLIENT_SEGMENT_ID_CHARS)
        .collect()
}

/// Times of the segments of one utterance (the model returns none): early sentences get an
/// estimated sub-range, the final remainder runs to the utterance end.
#[derive(Debug, Clone)]
pub struct UtteranceClock {
    pub seq: u64,
    pub start_ms: i64,
    pub end_ms: i64,
    last_end: i64,
    pub emitted: u32,
    pub text: String,
    /// Speaker label of the utterance (from its commit).
    pub speaker: Option<String>,
}

impl UtteranceClock {
    pub fn new(seq: u64, start_ms: i64, end_ms: i64) -> Self {
        let start_ms = start_ms.max(0);
        Self {
            seq,
            start_ms,
            end_ms: end_ms.max(start_ms),
            last_end: start_ms,
            emitted: 0,
            text: String::new(),
            speaker: None,
        }
    }

    /// `(start, end)` of the next segment; `early` = not the utterance's last.
    pub fn next(&mut self, text: &str, early: bool, est_ms_per_char: u64) -> (i64, i64) {
        let start = self.last_end.max(self.start_ms);
        let end = if early {
            let est = (text.chars().count() as u64).saturating_mul(est_ms_per_char);
            start
                .saturating_add(i64::try_from(est).unwrap_or(i64::MAX))
                .min(self.end_ms)
        } else {
            self.end_ms.max(start)
        };
        self.last_end = end;
        self.emitted += 1;
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(text);
        (start, end)
    }
}

/// Languages the server tells the provider about for a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguagePlan {
    pub mode: LanguageMode,
    /// Primary hint (`None` in `multi`).
    pub hint: Option<String>,
    /// All expected languages (base codes), primary first.
    pub languages: Vec<String>,
}

/// Mode + channel language + extra declared languages -> what to hint.
pub fn plan_languages(
    mode: LanguageMode,
    channel_language: &str,
    extra: &[String],
) -> LanguagePlan {
    let primary = super::base_language(channel_language);
    let mut languages = vec![primary.clone()];
    for l in extra {
        let b = super::base_language(l);
        if !b.is_empty() && !languages.contains(&b) {
            languages.push(b);
        }
    }
    let hint = match mode {
        LanguageMode::Multi => None,
        _ => Some(primary),
    };
    if mode == LanguageMode::Fixed {
        languages.truncate(1);
    }
    LanguagePlan {
        mode,
        hint,
        languages,
    }
}

/// Vocabulary prompt for providers that take one: deduplicated terms, capped by characters.
pub fn vocabulary_prompt(terms: &[String], max_terms: usize, max_chars: usize) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    let mut n = 0;
    for t in terms {
        let t = t.trim();
        if t.is_empty() || t.chars().count() > 60 || !seen.insert(t.to_lowercase()) {
            continue;
        }
        let add = if out.is_empty() { t.len() } else { t.len() + 2 };
        if n >= max_terms || out.chars().count() + add > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(t);
        n += 1;
    }
    (!out.is_empty()).then(|| format!("Glossary: {out}."))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_and_errors() {
        let f = encode_audio_frame(Channel::Others, &[1, -2, 300]);
        let (c, pcm) = parse_audio_frame(&f, 1000).unwrap();
        assert_eq!(c, Channel::Others);
        assert_eq!(pcm, vec![1, -2, 300]);
        assert_eq!(parse_audio_frame(&[], 10), Err(FrameError::Empty));
        assert_eq!(parse_audio_frame(&[0], 10), Err(FrameError::Empty));
        assert_eq!(
            parse_audio_frame(&[7, 0, 0], 10),
            Err(FrameError::UnknownChannel(7))
        );
        assert_eq!(parse_audio_frame(&[0, 1], 10), Err(FrameError::OddLength));
        assert_eq!(
            parse_audio_frame(&[0, 1, 2, 3, 4], 4),
            Err(FrameError::TooLarge(4))
        );
    }

    #[test]
    fn control_messages_parse() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"type":"commit","channel":"me","seq":3,"startMs":1000,"endMs":2500}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            ClientMsg::Commit {
                channel: Channel::Me,
                seq: 3,
                start_ms: 1000,
                end_ms: 2500,
                speaker: None
            }
        ));
        let b: ClientMsg = serde_json::from_str(
            r#"{"type":"commit","channel":"bot2","seq":1,"startMs":0,"endMs":900,"speaker":"Ana"}"#,
        )
        .unwrap();
        assert!(matches!(
            b,
            ClientMsg::Commit { channel: Channel::Bot(2), speaker: Some(ref n), .. } if n == "Ana"
        ));
        assert_eq!(
            parse_audio_frame(&[4, 1, 0], 64).unwrap().0,
            Channel::Bot(2)
        );
        assert!(serde_json::from_str::<ClientMsg>(r#"{"type":"commit","channel":"x"}"#).is_err());
        let h: ClientMsg =
            serde_json::from_str(r#"{"type":"hello","runId":"r1","vocabulary":["Kubernetes"]}"#)
                .unwrap();
        assert!(matches!(
            h,
            ClientMsg::Hello {
                run_id: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn utterance_clock_splits_the_range() {
        let mut c = UtteranceClock::new(0, 1000, 3000);
        let a = c.next("hello there", true, 65);
        assert_eq!(a, (1000, 1000 + 11 * 65));
        let b = c.next("rest", false, 65);
        assert_eq!(b, (a.1, 3000));
        assert_eq!(c.text, "hello there rest");
        // An estimate never runs past the utterance end.
        let mut c = UtteranceClock::new(1, 0, 100);
        assert_eq!(c.next(&"x".repeat(500), true, 65), (0, 100));
    }

    #[test]
    fn segment_ids_are_deterministic_and_bounded() {
        let a = server_segment_id("run1", Channel::Me, 4, 0);
        assert_eq!(a, server_segment_id("run1", Channel::Me, 4, 0));
        assert_ne!(a, server_segment_id("run1", Channel::Me, 4, 1));
        assert_ne!(a, server_segment_id("run1", Channel::Others, 4, 0));
        assert!(server_segment_id(&"r".repeat(500), Channel::Me, 1, 1).len() <= 100);
    }

    #[test]
    fn language_plans() {
        let extra = vec!["en".to_string(), "vi-VN".to_string()];
        let p = plan_languages(LanguageMode::Auto, "vi-VN", &extra);
        assert_eq!(p.hint.as_deref(), Some("vi"));
        assert_eq!(p.languages, vec!["vi", "en"]);
        assert_eq!(plan_languages(LanguageMode::Multi, "vi", &extra).hint, None);
        let f = plan_languages(LanguageMode::Fixed, "vi", &extra);
        assert_eq!((f.hint.as_deref(), f.languages.len()), (Some("vi"), 1));
    }

    #[test]
    fn vocabulary_prompt_dedups_and_caps() {
        let t: Vec<String> = ["Kubernetes", "kubernetes", " OKR ", "", "Sprint"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            vocabulary_prompt(&t, 10, 100).as_deref(),
            Some("Glossary: Kubernetes, OKR, Sprint.")
        );
        assert_eq!(
            vocabulary_prompt(&t, 1, 100).as_deref(),
            Some("Glossary: Kubernetes.")
        );
        assert_eq!(vocabulary_prompt(&[], 5, 100), None);
    }
}
