//! Audio route planning (spec: 10 §3.7). Pure: compares CONNECTED participants with current routes.

use std::collections::HashMap;

use uuid::Uuid;
use warptalk_kernel::ParticipantId;

/// A CONNECTED participant as seen by the planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpeaker {
    pub id: ParticipantId,
    pub speak_language: String,
    pub listen_language: String,
    /// Speaker has voice-clone consent and opted in.
    pub voice_clone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingRoute {
    pub id: Uuid,
    pub source: ParticipantId,
    pub target: ParticipantId,
    pub source_language: String,
    pub target_language: String,
    pub voice_clone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRoute {
    pub source: ParticipantId,
    pub target: ParticipantId,
    pub source_language: String,
    pub target_language: String,
    pub voice_clone: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    /// Current routes to close (departed participant, equal languages, or changed languages).
    pub remove: Vec<Uuid>,
    /// New routes (start PENDING).
    pub add: Vec<NewRoute>,
    /// Existing routes whose voice-clone flag changed.
    pub set_clone: Vec<(Uuid, bool)>,
}

impl RoutePlan {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.add.is_empty() && self.set_clone.is_empty()
    }
}

/// Wanted routes: every ordered pair (speaker, listener), speaker != listener, with different languages.
pub fn plan_routes(participants: &[RouteSpeaker], existing: &[ExistingRoute]) -> RoutePlan {
    let mut wanted: HashMap<(ParticipantId, ParticipantId), NewRoute> = HashMap::new();
    for s in participants {
        for l in participants {
            if s.id == l.id || s.speak_language == l.listen_language {
                continue;
            }
            wanted.insert(
                (s.id, l.id),
                NewRoute {
                    source: s.id,
                    target: l.id,
                    source_language: s.speak_language.clone(),
                    target_language: l.listen_language.clone(),
                    voice_clone: s.voice_clone,
                },
            );
        }
    }
    let mut plan = RoutePlan::default();
    for e in existing {
        match wanted.get(&(e.source, e.target)) {
            Some(w) if w.source_language == e.source_language && w.target_language == e.target_language => {
                if w.voice_clone != e.voice_clone {
                    plan.set_clone.push((e.id, w.voice_clone));
                }
                wanted.remove(&(e.source, e.target));
            }
            _ => plan.remove.push(e.id),
        }
    }
    let mut add: Vec<NewRoute> = wanted.into_values().collect();
    add.sort_by_key(|r| (r.source.as_uuid(), r.target.as_uuid()));
    plan.add = add;
    plan
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sp(speak: &str, listen: &str, clone: bool) -> RouteSpeaker {
        RouteSpeaker {
            id: ParticipantId::new(),
            speak_language: speak.into(),
            listen_language: listen.into(),
            voice_clone: clone,
        }
    }

    #[test]
    fn pairs_skip_equal_languages_and_self() {
        let a = sp("vi", "vi", false);
        let b = sp("en", "en", false);
        let c = sp("vi", "vi", false);
        let plan = plan_routes(&[a.clone(), b.clone(), c.clone()], &[]);
        // a->b, c->b, b->a, b->c ; a<->c same language
        assert_eq!(plan.add.len(), 4);
        assert!(plan.add.iter().all(|r| r.source != r.target));
        assert!(!plan.add.iter().any(|r| r.source == a.id && r.target == c.id));
    }

    #[test]
    fn departed_and_changed_are_replaced() {
        let a = sp("vi", "vi", false);
        let mut b = sp("en", "en", false);
        let first = plan_routes(&[a.clone(), b.clone()], &[]);
        let existing: Vec<ExistingRoute> = first
            .add
            .iter()
            .map(|r| ExistingRoute {
                id: Uuid::now_v7(),
                source: r.source,
                target: r.target,
                source_language: r.source_language.clone(),
                target_language: r.target_language.clone(),
                voice_clone: r.voice_clone,
            })
            .collect();
        assert!(plan_routes(&[a.clone(), b.clone()], &existing).is_empty());
        // b departs: everything removed
        let p = plan_routes(&[a.clone()], &existing);
        assert_eq!(p.remove.len(), 2);
        assert!(p.add.is_empty());
        // b now listens in vi: a->b disappears, b->a language unchanged
        b.listen_language = "vi".into();
        let p = plan_routes(&[a.clone(), b.clone()], &existing);
        assert_eq!(p.remove.len(), 1);
        assert!(p.add.is_empty());
        // b now speaks vi: b->a disappears; a->b target changed? listen vi != en -> removed above
        b.speak_language = "vi".into();
        let p = plan_routes(&[a, b], &existing);
        assert_eq!(p.remove.len(), 2);
        assert!(p.add.is_empty());
    }

    #[test]
    fn clone_flag_updates_in_place() {
        let a = sp("vi", "vi", true);
        let b = sp("en", "en", false);
        let existing = vec![ExistingRoute {
            id: Uuid::now_v7(),
            source: a.id,
            target: b.id,
            source_language: "vi".into(),
            target_language: "en".into(),
            voice_clone: false,
        }];
        let p = plan_routes(&[a, b], &existing);
        assert_eq!(p.set_clone, vec![(existing[0].id, true)]);
        assert_eq!(p.add.len(), 1); // b->a
        assert!(p.remove.is_empty());
    }
}
