//! Pure rules of copilot sessions: platforms, channels, segment validation, wire ids.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_contracts::events::SttResult;
use warptalk_kernel::RoomId;

pub mod audio;

/// Segments accepted per ingest call.
pub const MAX_SEGMENTS_PER_CALL: usize = 50;
/// Characters of one segment's text.
pub const MAX_SEGMENT_TEXT_CHARS: usize = 2000;
/// Characters of a client segment id.
pub const MAX_CLIENT_SEGMENT_ID_CHARS: usize = 100;
/// Default speaker names (ADR 0001: only "me" / "others" in the first phase).
pub const DEFAULT_NAME_ME: &str = "Tôi";
pub const DEFAULT_NAME_OTHERS: &str = "Others";
/// Default title of a session created without one.
pub const DEFAULT_TITLE: &str = "Copilot session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    GoogleMeet,
    Zoom,
    Teams,
    Other,
    InPerson,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GoogleMeet => "google_meet",
            Self::Zoom => "zoom",
            Self::Teams => "teams",
            Self::Other => "other",
            Self::InPerson => "in_person",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "google_meet" => Self::GoogleMeet,
            "zoom" => Self::Zoom,
            "teams" => Self::Teams,
            "other" => Self::Other,
            "in_person" => Self::InPerson,
            _ => return None,
        })
    }
}

/// Which audio the segment came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Microphone of the user ("Tôi").
    Me,
    /// Process tap of the meeting app (everybody else).
    Others,
    /// One of the meeting bot's received audio tracks (`0..BOT_CHANNELS`). Google Meet sends
    /// the three loudest speakers on three tracks; the bot labels each utterance with the
    /// speaker's name (commit `speaker`).
    Bot(u8),
}

/// Number of bot audio tracks the audio socket accepts.
pub const BOT_CHANNELS: u8 = 3;
/// Every channel of one audio socket (`me`, `others`, `bot0..bot2`).
pub const CHANNEL_COUNT: usize = 2 + BOT_CHANNELS as usize;
/// Longest speaker label kept on a segment.
pub const MAX_SPEAKER_NAME_CHARS: usize = 80;

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Me => "me",
            Self::Others => "others",
            Self::Bot(0) => "bot0",
            Self::Bot(1) => "bot1",
            Self::Bot(_) => "bot2",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "me" => Some(Self::Me),
            "others" => Some(Self::Others),
            "bot0" => Some(Self::Bot(0)),
            "bot1" => Some(Self::Bot(1)),
            "bot2" => Some(Self::Bot(2)),
            _ => None,
        }
    }

    /// Far side of the meeting (everything but the user's own microphone).
    pub fn is_far(&self) -> bool {
        !matches!(self, Self::Me)
    }
}

/// How the STT provider is told about the spoken language(s) (ADR 0001 language handling).
///
/// * `fixed`: one hinted language (the channel's declared one).
/// * `auto` (default for sessions with code-switching): the primary language is hinted, the other
///   declared languages are expected too and a vocabulary prompt carries the glossary. A single
///   hard hint on a Vietnamese meeting full of English product terms makes the model
///   transliterate the terms; pure auto-detection mis-detects short Vietnamese utterances.
/// * `multi`: no hint at all; the language of every segment is detected afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageMode {
    Fixed,
    #[default]
    Auto,
    Multi,
}

impl LanguageMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Auto => "auto",
            Self::Multi => "multi",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fixed" => Some(Self::Fixed),
            "auto" => Some(Self::Auto),
            "multi" => Some(Self::Multi),
            _ => None,
        }
    }
}

/// Base ISO code of a language tag (`vi-VN` -> `vi`), lowercase.
pub fn base_language(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// A language tag is 1..=15 ASCII letters, digits or `-`.
pub fn valid_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 15
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Stored session facts needed to build wire results.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub room_id: RoomId,
    pub host_participant_id: Uuid,
    pub others_speaker_id: Uuid,
    pub name_me: String,
    pub name_others: String,
    /// Start of the session: segment offsets are relative to it.
    pub started_at: DateTime<Utc>,
}

/// One validated client segment.
#[derive(Debug, Clone)]
pub struct Segment {
    pub client_segment_id: String,
    pub channel: Channel,
    pub text: String,
    pub language: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub confidence: Option<f64>,
    pub is_final: bool,
    /// Real name of the person speaking (meeting bot); `None` = the channel's default label.
    pub speaker_name: Option<String>,
}

/// Why a segment was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("segments[{index}]: {reason}")]
pub struct SegmentError {
    pub index: usize,
    pub reason: String,
}

impl Segment {
    /// Field rules shared by every segment (`index` only labels the error).
    pub fn validate(&self, index: usize) -> Result<(), SegmentError> {
        let bad = |reason: &str| SegmentError {
            index,
            reason: reason.to_owned(),
        };
        let id_len = self.client_segment_id.chars().count();
        if id_len == 0 || id_len > MAX_CLIENT_SEGMENT_ID_CHARS {
            return Err(bad("clientSegmentId must be 1-100 characters"));
        }
        if self.text.trim().is_empty() {
            return Err(bad("text must not be empty"));
        }
        if self.text.chars().count() > MAX_SEGMENT_TEXT_CHARS {
            return Err(bad("text must be at most 2000 characters"));
        }
        if !valid_language(&self.language) {
            return Err(bad(
                "language must be a language tag such as `en` or `vi-VN`",
            ));
        }
        let max = i64::from(i32::MAX);
        if self.start_ms < 0 || self.end_ms < self.start_ms || self.end_ms > max {
            return Err(bad(
                "startMs/endMs must satisfy 0 <= startMs <= endMs <= 2147483647",
            ));
        }
        if self.confidence.is_some_and(|c| !(0.0..=1.0).contains(&c)) {
            return Err(bad("confidence must be within 0..=1"));
        }
        Ok(())
    }
}

/// Deterministic wire segment id: UUID v5 of the client id in the room's namespace (36
/// characters, as the transcript requires), so a resend maps to the same transcript row.
pub fn segment_uuid(room: RoomId, client_segment_id: &str) -> Uuid {
    Uuid::new_v5(&room.as_uuid(), client_segment_id.as_bytes())
}

/// Stable speaker id of a named far-side participant (same name = same id within a room).
pub fn named_speaker_uuid(room: RoomId, name: &str) -> Uuid {
    let key = format!("copilot:speaker:{}", name.trim().to_lowercase());
    Uuid::new_v5(&room.as_uuid(), key.as_bytes())
}

/// Trimmed, length-capped speaker label; `None` when empty.
pub fn clean_speaker_name(name: Option<&str>) -> Option<String> {
    let n: String = name?
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_SPEAKER_NAME_CHARS)
        .collect();
    let n = n.trim().to_owned();
    (!n.is_empty()).then_some(n)
}

/// Fixed pseudo-speaker of the "others" channel of a session.
pub fn others_speaker_uuid(room: RoomId) -> Uuid {
    Uuid::new_v5(&room.as_uuid(), b"copilot:others")
}

/// The pipeline-contract result for a segment (spec 12 section 6.1, CONTRACT_AUDIT B-06/B-07):
/// offsets relative to the session start with `anchor_ms` = its epoch ms, the speaker's name in
/// `far_speaker_name` (source `participant`, confidence 1.0) so the transcript stores it.
pub fn to_stt_result(
    facts: &SessionFacts,
    seg: &Segment,
    now: DateTime<Utc>,
    chunk_index: i64,
) -> SttResult {
    let named = clean_speaker_name(seg.speaker_name.as_deref()).filter(|_| seg.channel.is_far());
    let (speaker_id, name) = match (seg.channel, named) {
        (Channel::Me, _) => (facts.host_participant_id, facts.name_me.clone()),
        (_, Some(n)) => (named_speaker_uuid(facts.room_id, &n), n),
        (_, None) => (facts.others_speaker_id, facts.name_others.clone()),
    };
    SttResult {
        segment_id: segment_uuid(facts.room_id, &seg.client_segment_id).to_string(),
        meeting_id: facts.room_id,
        speaker_id: speaker_id.to_string(),
        text: seg.text.trim().to_owned(),
        language: seg.language.clone(),
        start_ms: seg.start_ms,
        end_ms: seg.end_ms,
        anchor_ms: Some(facts.started_at.timestamp_millis()),
        confidence: seg.confidence,
        chunk_index: Some(chunk_index),
        is_final_chunk: seg.is_final,
        timestamp_ms: Some(now.timestamp_millis()),
        clean_flags: None,
        far_speaker_name: Some(name),
        far_speaker_source: Some("participant".to_owned()),
        far_speaker_confidence: Some(1.0),
        channel: Some(seg.channel.as_str().to_owned()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn seg() -> Segment {
        Segment {
            client_segment_id: "c1".into(),
            channel: Channel::Me,
            text: "hello".into(),
            language: "en".into(),
            start_ms: 10,
            end_ms: 20,
            confidence: Some(0.9),
            is_final: true,
            speaker_name: None,
        }
    }

    #[test]
    fn validation_rules() {
        assert!(seg().validate(0).is_ok());
        let mut s = seg();
        s.text = "x".repeat(2001);
        assert!(
            s.validate(3)
                .unwrap_err()
                .to_string()
                .starts_with("segments[3]")
        );
        let mut s = seg();
        s.end_ms = 5;
        assert!(s.validate(0).is_err());
        let mut s = seg();
        s.language = "a b".into();
        assert!(s.validate(0).is_err());
        let mut s = seg();
        s.confidence = Some(-0.5);
        assert!(s.validate(0).is_err());
        let mut s = seg();
        s.text = "  ".into();
        assert!(s.validate(0).is_err());
    }

    #[test]
    fn ids_are_deterministic_and_36_chars() {
        let room = RoomId::new();
        let a = segment_uuid(room, "x");
        assert_eq!(a, segment_uuid(room, "x"));
        assert_ne!(a, segment_uuid(room, "y"));
        assert_ne!(a, segment_uuid(RoomId::new(), "x"));
        assert_eq!(a.to_string().len(), 36);
        assert_ne!(
            others_speaker_uuid(room),
            others_speaker_uuid(RoomId::new())
        );
    }

    #[test]
    fn wire_result_follows_the_contract() {
        let room = RoomId::new();
        let started = DateTime::from_timestamp_millis(1_800_000_000_000).unwrap();
        let facts = SessionFacts {
            room_id: room,
            host_participant_id: Uuid::now_v7(),
            others_speaker_id: others_speaker_uuid(room),
            name_me: "Tôi".into(),
            name_others: "Others".into(),
            started_at: started,
        };
        let mut s = seg();
        s.channel = Channel::Others;
        let r = to_stt_result(&facts, &s, started, 0);
        assert_eq!(r.speaker_id, facts.others_speaker_id.to_string());
        assert_eq!(r.far_speaker_name.as_deref(), Some("Others"));
        assert_eq!(r.anchor_ms, Some(1_800_000_000_000));
        assert_eq!((r.start_ms, r.end_ms), (10, 20));
        assert!(r.is_final_chunk);
        assert_eq!(r.segment_id.len(), 36);
        let r = to_stt_result(&facts, &seg(), started, 0);
        assert_eq!(r.speaker_id, facts.host_participant_id.to_string());

        // Meeting bot: a named utterance gets the real name and a stable per-name id.
        let mut b = seg();
        b.channel = Channel::Bot(1);
        b.speaker_name = Some("  Tú Huỳnh ".into());
        let r = to_stt_result(&facts, &b, started, 0);
        assert_eq!(r.far_speaker_name.as_deref(), Some("Tú Huỳnh"));
        assert_eq!(r.channel.as_deref(), Some("bot1"));
        assert_eq!(
            r.speaker_id,
            named_speaker_uuid(room, "tú huỳnh").to_string()
        );
        assert_ne!(r.speaker_id, facts.others_speaker_id.to_string());
        // Unnamed bot utterance falls back to the "others" label.
        b.speaker_name = Some(" ".into());
        let r = to_stt_result(&facts, &b, started, 0);
        assert_eq!(r.speaker_id, facts.others_speaker_id.to_string());
        // The user's own microphone never takes a foreign name.
        let mut m = seg();
        m.speaker_name = Some("Someone".into());
        let r = to_stt_result(&facts, &m, started, 0);
        assert_eq!(r.far_speaker_name.as_deref(), Some("Tôi"));
    }

    #[test]
    fn bot_channels_round_trip() {
        for c in [
            Channel::Me,
            Channel::Others,
            Channel::Bot(0),
            Channel::Bot(1),
            Channel::Bot(2),
        ] {
            assert_eq!(Channel::parse(c.as_str()), Some(c));
            assert_eq!(Channel::from_tag(c.tag()), Some(c));
            assert!(c.index() < CHANNEL_COUNT);
        }
        assert_eq!(Channel::parse("bot3"), None);
        assert_eq!(Channel::from_tag(5), None);
        assert_eq!(
            clean_speaker_name(Some(&"x".repeat(200))).map(|n| n.chars().count()),
            Some(80)
        );
        assert_eq!(clean_speaker_name(Some("\n")), None);
    }
}
