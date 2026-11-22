//! What the room pipeline needs from an audio-routes snapshot (spec 10 section 3.7): which
//! participants are speakers (sources of a live route) and which target languages exist.

use std::collections::{BTreeMap, BTreeSet};

use warptalk_contracts::events::{AudioRoute, AudioRoutesSnapshot, RouteStatus};
use warptalk_pipeline_runtime::ParticipantId;

/// LiveKit identity of the external bridge stand-in seat (same value as the meeting module's
/// `BRIDGE_IDENTITY` and the room module's `STAND_IN_USER_ID`).
pub const BRIDGE_IDENTITY: &str = "00000000-0000-0000-0000-00000000b21d";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerRoutes {
    /// Declared spoken language of the speaker (the routes' `source_language`).
    pub source_language: String,
    /// Languages this speaker is translated into.
    pub target_languages: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoutesView {
    pub translation_active: bool,
    pub room_status: String,
    /// The room reached a terminal status; the owner should shut down.
    pub ended: bool,
    /// Speakers by participant id (identity of the LiveKit participant).
    pub speakers: BTreeMap<ParticipantId, SpeakerRoutes>,
    /// Union of target languages over live routes.
    pub targets: BTreeSet<String>,
    /// LiveKit identity -> speaker (participant id). Tokens are minted with identity = user id
    /// (the bridge stand-in with [`BRIDGE_IDENTITY`], which is the stand-in's user id), while
    /// routes are keyed by participant id. Identities without an entry are treated as
    /// participant ids (snapshots without user ids).
    pub identities: BTreeMap<String, ParticipantId>,
    /// Participant id -> user id: the key of the per-user voice consent / profile state.
    pub users: BTreeMap<ParticipantId, String>,
    /// Live routes (paused/ending/completed ones are filtered out).
    pub routes: Vec<AudioRoute>,
}

/// Route states that no longer need audio processed.
fn is_live(status: RouteStatus) -> bool {
    !matches!(
        status,
        RouteStatus::Paused
            | RouteStatus::Ending
            | RouteStatus::SavingOutputs
            | RouteStatus::Completed
    )
}

/// `room_status` strings that mean the room is over. The spec does not list the enum
/// (10 section 3.7), so this is matched case-insensitively on the common names.
fn is_terminal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "ended" | "cancelled" | "canceled" | "completed" | "closed" | "finished"
    )
}

impl RoutesView {
    pub fn from_snapshot(s: &AudioRoutesSnapshot) -> Self {
        let mut speakers: BTreeMap<ParticipantId, SpeakerRoutes> = BTreeMap::new();
        let mut targets = BTreeSet::new();
        let mut routes = Vec::new();
        let mut identities = BTreeMap::new();
        let mut users = BTreeMap::new();
        for r in s.routes.iter().filter(|r| is_live(r.status)) {
            let sp = speakers
                .entry(r.source_participant_id.to_string())
                .or_insert_with(|| SpeakerRoutes {
                    source_language: r.source_language.clone(),
                    target_languages: BTreeSet::new(),
                });
            sp.target_languages.insert(r.target_language.clone());
            if let Some(u) = r.source_user_id {
                let (pid, uid) = (r.source_participant_id.to_string(), u.to_string());
                identities.insert(uid.clone(), pid.clone());
                users.insert(pid, uid);
            }
            targets.insert(r.target_language.clone());
            routes.push(r.clone());
        }
        Self {
            translation_active: s.translation_active,
            ended: is_terminal(&s.room_status),
            room_status: s.room_status.clone(),
            speakers,
            identities,
            users,
            targets,
            routes,
        }
    }

    /// The speaker (participant id) a LiveKit identity belongs to.
    pub fn speaker_for_identity<'a>(&'a self, identity: &'a str) -> &'a str {
        self.identities
            .get(identity)
            .map_or(identity, String::as_str)
    }

    /// The user id of a speaker, for consent lookups (the speaker itself when unknown).
    pub fn user_of<'a>(&'a self, speaker: &'a str) -> &'a str {
        self.users.get(speaker).map_or(speaker, String::as_str)
    }

    /// Speakers whose audio should be transcribed right now.
    pub fn active_speakers(&self) -> BTreeMap<&ParticipantId, &SpeakerRoutes> {
        if self.translation_active && !self.ended {
            self.speakers.iter().collect()
        } else {
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;
    use warptalk_kernel::ParticipantId as Pid;

    fn route(src: Pid, lang_from: &str, lang_to: &str, status: RouteStatus) -> AudioRoute {
        AudioRoute {
            route_id: Uuid::now_v7(),
            source_participant_id: src,
            target_participant_id: Pid::new(),
            source_user_id: None,
            target_user_id: None,
            source_language: lang_from.into(),
            target_language: lang_to.into(),
            voice_clone_enabled: false,
            status,
        }
    }

    fn snap(routes: Vec<AudioRoute>, active: bool, status: &str) -> AudioRoutesSnapshot {
        AudioRoutesSnapshot {
            generated_at: Utc::now(),
            room_status: status.into(),
            translation_active: active,
            room_languages: vec!["en".into(), "vi".into()],
            routes,
        }
    }

    #[test]
    fn groups_routes_by_speaker_and_drops_dead_ones() {
        let (a, b) = (Pid::new(), Pid::new());
        let v = RoutesView::from_snapshot(&snap(
            vec![
                route(a, "en", "vi", RouteStatus::Broadcasting),
                route(a, "en", "ja", RouteStatus::Pending),
                route(b, "vi", "en", RouteStatus::Paused),
            ],
            true,
            "Active",
        ));
        assert_eq!(v.speakers.len(), 1);
        let sp = &v.speakers[&a.to_string()];
        assert_eq!(sp.source_language, "en");
        assert_eq!(sp.target_languages.len(), 2);
        assert_eq!(v.targets.len(), 2);
        assert_eq!(v.active_speakers().len(), 1);
        assert!(!v.ended);
    }

    #[test]
    fn identities_map_to_participants_incl_bridge_and_fall_back() {
        let (a, b, c) = (Pid::new(), Pid::new(), Pid::new());
        let (ua, bridge) = (
            warptalk_kernel::UserId::new(),
            BRIDGE_IDENTITY.parse().unwrap(),
        );
        let mut ra = route(a, "en", "vi", RouteStatus::Broadcasting);
        ra.source_user_id = Some(ua);
        let mut rb = route(b, "vi", "en", RouteStatus::Broadcasting);
        rb.source_user_id = Some(warptalk_kernel::UserId::from_uuid(bridge));
        let rc = route(c, "en", "vi", RouteStatus::Broadcasting); // no user id
        let v = RoutesView::from_snapshot(&snap(vec![ra, rb, rc], true, "Active"));
        assert_eq!(v.speaker_for_identity(&ua.to_string()), a.to_string());
        assert_eq!(v.speaker_for_identity(BRIDGE_IDENTITY), b.to_string());
        // no user id: the identity is taken as the participant id, as before
        assert_eq!(v.speaker_for_identity(&c.to_string()), c.to_string());
        assert_eq!(v.user_of(&a.to_string()), ua.to_string());
        assert_eq!(v.user_of(&b.to_string()), BRIDGE_IDENTITY);
        assert_eq!(v.user_of(&c.to_string()), c.to_string());
    }

    #[test]
    fn inactive_translation_has_no_active_speakers_and_ended_is_detected() {
        let a = Pid::new();
        let v = RoutesView::from_snapshot(&snap(
            vec![route(a, "en", "vi", RouteStatus::Ready)],
            false,
            "Active",
        ));
        assert!(v.active_speakers().is_empty());
        let v = RoutesView::from_snapshot(&snap(vec![], true, "Ended"));
        assert!(v.ended);
    }
}
