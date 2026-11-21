//! Vector-store filters for a search: workspace + retrieval-state gate, caller filters and the
//! per-document / per-meeting allowlist scope.

use warptalk_qdrant::{Condition, Filter};

use super::model::{AccessContext, EffectiveScope, SearchFilters};
use crate::domain::{SourceType, fields, normalize_id};

/// Allowlist scope as a nested filter (spec 14 section 4.3): sources without a scoped dimension
/// (glossary, other types) pass; documents need `source_id` in the document list; transcripts
/// and meeting summaries need `meeting_id` in the meeting list. `None` leaves a dimension
/// unscoped; an empty list matches nothing.
pub fn scope_filter(scope: &EffectiveScope) -> Option<Filter> {
    let mut scoped_types: Vec<&str> = Vec::new();
    let mut branches: Vec<Condition> = Vec::new();
    if let Some(docs) = &scope.documents {
        scoped_types.push(SourceType::Document.as_str());
        branches.push(Condition::Nested(Box::new(Filter::must(vec![
            Condition::eq(fields::SOURCE_TYPE, SourceType::Document.as_str()),
            Condition::any_of(fields::SOURCE_ID, docs.iter().cloned()),
        ]))));
    }
    if let Some(meetings) = &scope.meetings {
        for st in [SourceType::Transcript, SourceType::MeetingSummary] {
            scoped_types.push(st.as_str());
            branches.push(Condition::Nested(Box::new(Filter::must(vec![
                Condition::eq(fields::SOURCE_TYPE, st.as_str()),
                Condition::any_of(fields::MEETING_ID, meetings.iter().cloned()),
            ]))));
        }
    }
    if scoped_types.is_empty() {
        return None;
    }
    // "not one of the scoped types" keeps unscoped types reachable by default.
    branches.insert(
        0,
        Condition::Nested(Box::new(Filter {
            must_not: vec![Condition::any_of(
                fields::SOURCE_TYPE,
                scoped_types.iter().map(|s| (*s).to_owned()),
            )],
            ..Filter::default()
        })),
    );
    Some(Filter {
        should: branches,
        ..Filter::default()
    })
}

/// Full query filter. Always pins the workspace and only returns active, AI-retrievable chunks.
pub fn search_filter(ctx: &AccessContext, scope: &EffectiveScope, f: &SearchFilters) -> Filter {
    let mut must = vec![
        Condition::eq(fields::WORKSPACE_ID, ctx.workspace.to_string()),
        Condition::eq(fields::AI_RETRIEVAL, true),
        Condition::eq(fields::RETENTION_STATE, "active"),
        Condition::eq(fields::DELETION_STATE, "active"),
    ];
    if !f.source_types.is_empty() {
        must.push(Condition::any_of(
            fields::SOURCE_TYPE,
            f.source_types.iter().map(|s| s.as_str().to_owned()),
        ));
    }
    if let Some(p) = f.page {
        must.push(Condition::eq(fields::PAGE, p));
    }
    if let Some(id) = &f.source_id {
        must.push(Condition::eq(fields::SOURCE_ID, normalize_id(id)));
    }
    if let Some(scope) = scope_filter(scope) {
        must.push(Condition::Nested(Box::new(scope)));
    }
    Filter::must(must)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;
    use warptalk_kernel::{UserId, WorkspaceId};

    #[test]
    fn scope_semantics_match_the_allowlist_rules() {
        let doc_ok = Uuid::now_v7().to_string();
        let scope = EffectiveScope {
            documents: Some(vec![doc_ok.clone()]),
            meetings: Some(vec![]),
        };
        let f = scope_filter(&scope).unwrap();
        let glossary = json!({"source_type": "glossary_term", "source_id": "t"});
        let allowed_doc = json!({"source_type": "document", "source_id": doc_ok});
        let other_doc = json!({"source_type": "document", "source_id": "nope"});
        let transcript = json!({"source_type": "transcript", "meeting_id": "m1"});
        assert!(f.matches("x", &glossary));
        assert!(f.matches("x", &allowed_doc));
        assert!(!f.matches("x", &other_doc));
        assert!(
            !f.matches("x", &transcript),
            "empty meeting list matches nothing"
        );
        assert!(
            scope_filter(&EffectiveScope {
                documents: None,
                meetings: None
            })
            .is_none()
        );
    }

    #[test]
    fn full_filter_pins_workspace_and_state() {
        let w = WorkspaceId::new();
        let ctx =
            AccessContext::privileged(w, UserId::new(), warptalk_kernel::WorkspaceRole::Owner);
        let f = search_filter(
            &ctx,
            &ctx.effective_scope(),
            &SearchFilters {
                page: Some(3),
                ..SearchFilters::default()
            },
        );
        let ok = json!({"workspace_id": w.to_string(), "ai_retrieval": true,
            "retention_state": "active", "deletion_state": "active", "page": 3});
        assert!(f.matches("x", &ok));
        for (k, v) in [
            ("workspace_id", json!("other")),
            ("ai_retrieval", json!(false)),
            ("retention_state", json!("expired")),
            ("deletion_state", json!("deleted")),
            ("page", json!(4)),
        ] {
            let mut p = ok.clone();
            p[k] = v;
            assert!(!f.matches("x", &p), "{k}");
        }
    }
}
