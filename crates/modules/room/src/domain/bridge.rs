//! External bridge rules (docs/architecture/external-meeting.md; spec 10 section 3.8). Pure.

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Stored `translation_room_type` of a bridge room.
pub const ROOM_TYPE_EXTERNAL_BRIDGE: &str = "EXTERNAL_BRIDGE";

/// Fixed user id (and LiveKit identity) of the far-side stand-in seat.
pub const STAND_IN_USER_ID: Uuid = Uuid::from_u128(0xb21d);
/// Roster name of the stand-in seat.
pub const STAND_IN_DISPLAY_NAME: &str = "External Meeting";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomType {
    Native,
    ExternalBridge,
}

impl RoomType {
    /// Exact match only; anything else (GROUP, EVENT, ...) is a native room.
    pub fn parse(s: &str) -> Self {
        if s == ROOM_TYPE_EXTERNAL_BRIDGE {
            Self::ExternalBridge
        } else {
            Self::Native
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "NATIVE",
            Self::ExternalBridge => ROOM_TYPE_EXTERNAL_BRIDGE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeProvider {
    GoogleMeet,
    Zoom,
    Teams,
}

impl BridgeProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GoogleMeet => "google_meet",
            Self::Zoom => "zoom",
            Self::Teams => "teams",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "google_meet" => Self::GoogleMeet,
            "zoom" => Self::Zoom,
            "teams" => Self::Teams,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    Voice,
    Text,
}

impl AudioMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Voice => "voice",
            Self::Text => "text",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "voice" => Self::Voice,
            "text" => Self::Text,
            _ => return None,
        })
    }
}

/// Bridge timings (later: settings namespace). Defaults per spec 10 section 3.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeConfig {
    pub heartbeat_interval: Duration,
    pub stale_after: Duration,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(15),
            stale_after: Duration::from_secs(45),
        }
    }
}

impl BridgeConfig {
    fn stale_chrono(&self) -> chrono::Duration {
        chrono::Duration::from_std(self.stale_after).unwrap_or(chrono::Duration::seconds(45))
    }
}

/// Capturer lease state of a bridge room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturerLease {
    pub user_id: Option<Uuid>,
    pub lease_at: Option<DateTime<Utc>>,
    /// Last heartbeat report (proposal B8). `None`: nothing reported yet.
    pub audio_flowing: Option<bool>,
    pub last_audio_at: Option<DateTime<Utc>>,
}

impl CapturerLease {
    /// A capturer is stale when its heartbeat is older than `stale_after`, or when its heartbeat
    /// keeps reporting no audio and the last audio is older than `stale_after` (proposal B8).
    /// A room without capturer is not stale (there is nothing to take over).
    pub fn is_stale(&self, now: DateTime<Utc>, cfg: &BridgeConfig) -> bool {
        if self.user_id.is_none() {
            return false;
        }
        let window = cfg.stale_chrono();
        match self.lease_at {
            Some(t) if now - t <= window => {}
            _ => return true,
        }
        if self.audio_flowing == Some(false) {
            return match self.last_audio_at.or(self.lease_at) {
                Some(t) => now - t > window,
                None => true,
            };
        }
        false
    }
}

/// Bridge metadata of an EXTERNAL_BRIDGE room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeState {
    pub provider: BridgeProvider,
    pub meeting_code: String,
    pub join_url: Option<String>,
    pub lease: CapturerLease,
    pub audio_mode: AudioMode,
    /// Incremented on every takeover (a new capturer invalidates the previous stand-in token).
    pub epoch: i32,
}

/// Meeting reference was not understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidMeetingRef(pub &'static str);

fn is_meet_code(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 3
        && [3usize, 4, 3]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_lowercase()))
}

/// Canonical meeting key for `(workspace, provider, key)` idempotency.
/// Google Meet: `abc-defg-hij` from a code or a `meet.google.com/...` URL (query ignored).
/// Zoom: the meeting number (digits) from a number or a `/j/<number>` URL.
/// Teams: the join URL without query/fragment (scheme and host lowercased).
pub fn normalize_meeting_ref(
    provider: BridgeProvider,
    input: &str,
) -> Result<String, InvalidMeetingRef> {
    let input = input.trim();
    if input.is_empty() {
        return Err(InvalidMeetingRef("meetingUrl or code is required"));
    }
    let is_url = input.contains('/') || input.contains('.');
    let no_query = input.split(['?', '#']).next().unwrap_or(input);
    let path_after_host = |s: &str| -> String {
        let s = s
            .strip_prefix("https://")
            .or_else(|| s.strip_prefix("http://"))
            .unwrap_or(s);
        s.to_owned()
    };
    match provider {
        BridgeProvider::GoogleMeet => {
            let candidate = if is_url {
                let rest = path_after_host(no_query);
                let (host, path) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
                if !host.eq_ignore_ascii_case("meet.google.com") {
                    return Err(InvalidMeetingRef("not a Google Meet URL"));
                }
                path.trim_matches('/').to_ascii_lowercase()
            } else {
                no_query.to_ascii_lowercase()
            };
            if is_meet_code(&candidate) {
                Ok(candidate)
            } else {
                Err(InvalidMeetingRef(
                    "Google Meet code must look like abc-defg-hij",
                ))
            }
        }
        BridgeProvider::Zoom => {
            let digits: String = if is_url {
                let rest = path_after_host(no_query);
                let (host, path) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
                if !(host.eq_ignore_ascii_case("zoom.us")
                    || host.to_ascii_lowercase().ends_with(".zoom.us"))
                {
                    return Err(InvalidMeetingRef("not a Zoom URL"));
                }
                path.trim_start_matches("j/").trim_matches('/').to_owned()
            } else {
                no_query.chars().filter(|c| !c.is_whitespace()).collect()
            };
            if (9..=11).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                Ok(digits)
            } else {
                Err(InvalidMeetingRef("Zoom meeting number must be 9-11 digits"))
            }
        }
        BridgeProvider::Teams => {
            if !is_url {
                return Err(InvalidMeetingRef("Teams meetings need the join URL"));
            }
            let rest = path_after_host(no_query);
            let (host, path) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
            let host = host.to_ascii_lowercase();
            if !(host == "teams.microsoft.com" || host == "teams.live.com") || path.is_empty() {
                return Err(InvalidMeetingRef("not a Microsoft Teams join URL"));
            }
            Ok(format!("https://{host}/{}", path.trim_end_matches('/')))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(s: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + s, 0).unwrap()
    }

    fn lease(lease_at: i64, flowing: Option<bool>, last_audio: Option<i64>) -> CapturerLease {
        CapturerLease {
            user_id: Some(Uuid::nil()),
            lease_at: Some(t(lease_at)),
            audio_flowing: flowing,
            last_audio_at: last_audio.map(t),
        }
    }

    #[test]
    fn stale_by_heartbeat_age() {
        let cfg = BridgeConfig::default();
        let l = lease(0, Some(true), Some(0));
        assert!(!l.is_stale(t(45), &cfg));
        assert!(l.is_stale(t(46), &cfg));
    }

    #[test]
    fn stale_by_silent_audio_with_live_heartbeat() {
        let cfg = BridgeConfig::default();
        // heartbeat is fresh (at 100) but audio stopped at 0
        let l = lease(100, Some(false), Some(0));
        assert!(l.is_stale(t(101), &cfg));
        // audio stopped recently: still fine
        let l = lease(100, Some(false), Some(80));
        assert!(!l.is_stale(t(101), &cfg));
        // audio flowing never stale while heartbeating
        let l = lease(100, Some(true), Some(0));
        assert!(!l.is_stale(t(101), &cfg));
    }

    #[test]
    fn no_capturer_is_not_stale() {
        let l = CapturerLease {
            user_id: None,
            lease_at: None,
            audio_flowing: None,
            last_audio_at: None,
        };
        assert!(!l.is_stale(t(1000), &BridgeConfig::default()));
    }

    #[test]
    fn room_type_and_enums() {
        assert_eq!(RoomType::parse("EXTERNAL_BRIDGE"), RoomType::ExternalBridge);
        assert_eq!(RoomType::parse("external_bridge"), RoomType::Native);
        assert_eq!(RoomType::parse("GROUP"), RoomType::Native);
        assert_eq!(BridgeProvider::parse("zoom"), Some(BridgeProvider::Zoom));
        assert_eq!(BridgeProvider::parse("meet"), None);
        assert_eq!(AudioMode::parse("text"), Some(AudioMode::Text));
        assert_eq!(
            STAND_IN_USER_ID.to_string(),
            "00000000-0000-0000-0000-00000000b21d"
        );
    }

    #[test]
    fn meet_refs() {
        let p = BridgeProvider::GoogleMeet;
        for i in [
            "abc-defg-hij",
            " ABC-DEFG-HIJ ",
            "https://meet.google.com/abc-defg-hij",
            "meet.google.com/abc-defg-hij?authuser=1",
        ] {
            assert_eq!(normalize_meeting_ref(p, i).unwrap(), "abc-defg-hij", "{i}");
        }
        for i in [
            "",
            "abcdefghij",
            "https://example.com/abc-defg-hij",
            "ab-defg-hij",
        ] {
            assert!(normalize_meeting_ref(p, i).is_err(), "{i}");
        }
    }

    #[test]
    fn zoom_and_teams_refs() {
        let z = BridgeProvider::Zoom;
        assert_eq!(
            normalize_meeting_ref(z, "123 4567 8901").unwrap(),
            "12345678901"
        );
        assert_eq!(
            normalize_meeting_ref(z, "https://us02web.zoom.us/j/12345678901?pwd=x").unwrap(),
            "12345678901"
        );
        assert!(normalize_meeting_ref(z, "12").is_err());
        let tm = BridgeProvider::Teams;
        assert_eq!(
            normalize_meeting_ref(
                tm,
                "https://Teams.Microsoft.com/l/meetup-join/19%3Aabc/0?context=1"
            )
            .unwrap(),
            "https://teams.microsoft.com/l/meetup-join/19%3Aabc/0"
        );
        assert!(normalize_meeting_ref(tm, "abc").is_err());
    }
}
