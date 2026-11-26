#![allow(clippy::unwrap_used)]
//! Suggestion feedback loop: RBAC, persistence, quality aggregate and the ai-eval export shape,
//! against real Postgres. Skips when DATABASE_URL is unset.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_assistant::application::ports::WorkspaceAccess;
use warptalk_assistant::suggestion::PgSuggestionStore;
use warptalk_assistant::suggestion::feedback_http::{FeedbackApi, router};
use warptalk_assistant::suggestion::store::{ContextLine, SuggestionRecord, SuggestionStore};
use warptalk_assistant::summary::{RoomAccess, RoomAccessInfo, WorkspaceScope};
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;

#[derive(Default)]
struct FakeAccess {
    rooms: Mutex<HashMap<(RoomId, UserId), RoomAccessInfo>>,
    roles: Mutex<HashMap<(WorkspaceId, UserId), WorkspaceRole>>,
}

#[async_trait]
impl RoomAccess for FakeAccess {
    async fn room_access(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomAccessInfo>> {
        Ok(self.rooms.lock().unwrap().get(&(room, user)).cloned())
    }
    async fn workspace_scope(
        &self,
        _: WorkspaceId,
        _: UserId,
    ) -> AppResult<Option<WorkspaceScope>> {
        Ok(None)
    }
}

#[async_trait]
impl WorkspaceAccess for FakeAccess {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(self.roles.lock().unwrap().get(&(ws, user)).copied())
    }
}

/// Local mirror of the `warptalk-ai-eval` dataset types (`apps/ai-eval/src/dataset.rs`); the
/// assistant crate cannot depend on ai-eval (it depends on us).
mod eval_mirror {
    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Channel {
        Me,
        Others,
        Unknown,
    }
    #[derive(Debug, Deserialize)]
    pub struct Line {
        pub t_ms: i64,
        #[allow(dead_code)]
        pub channel: Channel,
        #[allow(dead_code)]
        pub speaker: String,
        pub text: String,
    }
    #[derive(Debug, Deserialize)]
    pub struct Snippet {
        #[allow(dead_code)]
        pub title: String,
    }
    #[derive(Debug, Deserialize)]
    pub struct Forbidden {
        pub at_line: usize,
        pub category: String,
    }
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    pub struct SuggestionExpect {
        pub forbidden: Vec<Forbidden>,
        pub must_not_include: Vec<String>,
    }
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    pub struct Expectations {
        pub suggestions: SuggestionExpect,
    }
    #[derive(Debug, Deserialize)]
    pub struct Meeting {
        pub id: String,
        pub language: String,
        pub title: String,
        #[serde(default)]
        pub participants: Vec<String>,
        pub lines: Vec<Line>,
        #[serde(default)]
        pub snippets: Vec<Snippet>,
        #[serde(default)]
        pub expectations: Expectations,
    }

    /// The same checks as `Meeting::validate`.
    pub fn validate(m: &Meeting) {
        assert!(!m.id.trim().is_empty() && !m.lines.is_empty());
        let mut prev = i64::MIN;
        let mut texts = HashSet::new();
        for l in &m.lines {
            assert!(l.t_ms >= prev, "goes back in time");
            prev = l.t_ms;
            assert!(texts.insert(l.text.clone()), "repeated text");
        }
        for f in &m.expectations.suggestions.forbidden {
            assert!(f.at_line < m.lines.len());
        }
    }
}

struct H {
    pool: sqlx::PgPool,
    store: Arc<PgSuggestionStore>,
    access: Arc<FakeAccess>,
    jwt: JwtKeys,
    ws: WorkspaceId,
    room: RoomId,
}

async fn harness() -> Option<H> {
    let Ok(db) = std::env::var("DATABASE_URL") else {
        println!("skipping: DATABASE_URL not set");
        return None;
    };
    let pool = warptalk_db::connect(&db, 2).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_assistant::MIGRATIONS])
        .await
        .unwrap();
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    Some(H {
        store: Arc::new(PgSuggestionStore::new(pool.clone())),
        pool,
        access: Arc::new(FakeAccess::default()),
        jwt,
        ws: WorkspaceId::new(),
        room: RoomId::new(),
    })
}

impl H {
    fn app(&self) -> Router {
        router(
            FeedbackApi {
                store: self.store.clone(),
                access: self.access.clone(),
                members: self.access.clone(),
                clock: Arc::new(SystemClock),
            },
            self.jwt.clone(),
        )
    }

    fn token(&self, user: UserId, sys: bool) -> String {
        self.jwt
            .issue_access(user, Uuid::new_v4(), sys, Utc::now())
            .unwrap()
            .0
    }

    fn grant(&self, user: UserId, host: bool, participant: bool, role: Option<WorkspaceRole>) {
        self.access.rooms.lock().unwrap().insert(
            (self.room, user),
            RoomAccessInfo {
                workspace_id: self.ws,
                is_host: host,
                is_participant: participant,
                workspace_role: role,
            },
        );
        if let Some(r) = role {
            self.access.roles.lock().unwrap().insert((self.ws, user), r);
        }
    }

    async fn seed(&self, category: &str, content: &str) -> Uuid {
        let id = Uuid::now_v7();
        self.store
            .record(&SuggestionRecord {
                id,
                workspace_id: self.ws.as_uuid(),
                room_id: self.room.as_uuid(),
                segment_id: "seg".into(),
                category: category.into(),
                severity: "info".into(),
                content: content.into(),
                detail: String::new(),
                language: "en".into(),
                model: "m1".into(),
                prompt_version: "builtin-1".into(),
                confidence: 0.8,
                sources: json!([]),
                context: vec![
                    ContextLine {
                        t_ms: 0,
                        channel: "others".into(),
                        speaker: "Alice Nguyen".into(),
                        text: "mail me at alice@corp.com about the budget".into(),
                    },
                    ContextLine {
                        t_ms: 1000,
                        channel: "me".into(),
                        speaker: "Bob".into(),
                        text: "mail me at alice@corp.com about the budget".into(),
                    },
                    ContextLine {
                        t_ms: 900,
                        channel: "others".into(),
                        speaker: "Alice Nguyen".into(),
                        text: "so what is the number".into(),
                    },
                ],
            })
            .await
            .unwrap();
        id
    }

    async fn call(
        &self,
        user: UserId,
        sys: bool,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token(user, sys)));
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.app().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn feedback_rows(&self, id: Uuid) -> Vec<(Uuid, String, Option<String>)> {
        sqlx::query_as(
            "SELECT user_id, action, comment FROM assistant.suggestion_feedback \
             WHERE suggestion_id = $1 ORDER BY action",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

fn range() -> String {
    let f = |t: chrono::DateTime<Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    format!(
        "from={}&to={}",
        f(Utc::now() - Duration::days(1)),
        f(Utc::now() + Duration::days(1))
    )
}

#[tokio::test]
async fn feedback_requires_a_viewer_of_the_suggestions_room() {
    let Some(h) = harness().await else { return };
    let (host, guest, outsider, stranger) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    h.grant(host, true, true, None);
    h.grant(guest, false, true, Some(WorkspaceRole::Member));
    // Knows the room but cannot view it (workspace member, not in the room).
    h.grant(outsider, false, false, Some(WorkspaceRole::Member));
    let id = h.seed("term", "Next.js is a React framework.").await;
    let uri = format!("/suggestions/{id}/feedback");

    let (st, _) = h
        .call(host, false, "POST", &uri, Some(json!({"action": "used"})))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = h
        .call(
            guest,
            false,
            "POST",
            &uri,
            Some(json!({"action": "wrong", "comment": "  bad\u{7} date  "})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = h
        .call(
            outsider,
            false,
            "POST",
            &uri,
            Some(json!({"action": "helpful"})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = h
        .call(
            stranger,
            false,
            "POST",
            &uri,
            Some(json!({"action": "helpful"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = h
        .call(
            host,
            false,
            "POST",
            &format!("/suggestions/{}/feedback", Uuid::now_v7()),
            Some(json!({"action": "used"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = h
        .call(
            host,
            false,
            "POST",
            &uri,
            Some(json!({"action": "nonsense"})),
        )
        .await;
    assert!(st.is_client_error());
    // Unauthenticated.
    let resp = h
        .app()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(&uri)
                .header("content-type", "application/json")
                .body(Body::from(json!({"action": "used"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let rows = h.feedback_rows(id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].1, "wrong");
    assert_eq!(rows[1].2.as_deref(), Some("bad date"));

    // Repeating a verdict updates it instead of duplicating.
    let (st, _) = h
        .call(
            guest,
            false,
            "POST",
            &uri,
            Some(json!({"action": "wrong", "comment": "still wrong"})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let rows = h.feedback_rows(id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].2.as_deref(), Some("still wrong"));
}

#[tokio::test]
async fn quality_aggregates_per_category_for_owner_and_admin_only() {
    let Some(h) = harness().await else { return };
    let (owner, member) = (UserId::new(), UserId::new());
    h.grant(owner, false, true, Some(WorkspaceRole::Owner));
    h.grant(member, false, true, Some(WorkspaceRole::Member));
    let (t1, t2, t3, a1) = (
        h.seed("term", "one").await,
        h.seed("term", "two").await,
        h.seed("term", "three").await,
        h.seed("answer", "four").await,
    );
    let fb = |id: Uuid, user: UserId, action: &str| {
        let uri = format!("/suggestions/{id}/feedback");
        let action = action.to_string();
        let h = &h;
        async move {
            let (st, _) = h
                .call(user, false, "POST", &uri, Some(json!({ "action": action })))
                .await;
            assert_eq!(st, StatusCode::NO_CONTENT);
        }
    };
    fb(t1, owner, "used").await;
    fb(t1, member, "helpful").await;
    fb(t2, owner, "dismissed").await;
    fb(t2, member, "wrong").await;
    fb(t3, member, "wrong").await;
    fb(t3, owner, "wrong").await; // two users, still one wrong suggestion
    fb(a1, owner, "not_helpful").await;

    let uri = format!("/workspaces/{}/suggestions/quality?{}", h.ws, range());
    let (st, v) = h.call(owner, false, "GET", &uri, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let cats = v["categories"].as_array().unwrap();
    let term = cats.iter().find(|c| c["category"] == "term").unwrap();
    assert_eq!(
        (
            term["shown"].as_i64(),
            term["used"].as_i64(),
            term["dismissed"].as_i64(),
            term["wrong"].as_i64()
        ),
        (Some(3), Some(1), Some(1), Some(2))
    );
    assert!((term["wrongRate"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    let answer = cats.iter().find(|c| c["category"] == "answer").unwrap();
    assert_eq!(
        (
            answer["shown"].as_i64(),
            answer["notHelpful"].as_i64(),
            answer["wrongRate"].as_f64()
        ),
        (Some(1), Some(1), Some(0.0))
    );
    assert_eq!(v["total"]["shown"], 4);
    assert_eq!(v["total"]["wrong"], 2);

    // Plain member: forbidden. Other workspace of the owner: forbidden (no role there).
    let (st, _) = h.call(member, false, "GET", &uri, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let other = format!(
        "/workspaces/{}/suggestions/quality?{}",
        Uuid::now_v7(),
        range()
    );
    let (st, _) = h.call(owner, false, "GET", &other, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // A system admin may read it; an inverted range is rejected.
    let (st, v) = h.call(member, true, "GET", &other, None).await;
    assert_eq!(
        (st, v["total"]["shown"].as_i64()),
        (StatusCode::OK, Some(0))
    );
    let bad = format!(
        "/workspaces/{}/suggestions/quality?from=2026-02-01&to=2026-01-01",
        h.ws
    );
    let (st, _) = h.call(owner, false, "GET", &bad, None).await;
    assert!(st.is_client_error());
}

#[tokio::test]
async fn export_is_system_admin_only_anonymised_and_parses_as_an_eval_meeting() {
    let Some(h) = harness().await else { return };
    let (host, admin) = (UserId::new(), UserId::new());
    h.grant(host, true, true, None);
    let bad = h.seed("fact", "The budget is 9M, see http://x.test").await;
    let meh = h.seed("term", "meh hint").await;
    let fine = h.seed("term", "fine hint").await;
    for (id, action) in [(bad, "wrong"), (meh, "not_helpful"), (fine, "helpful")] {
        let (st, _) = h
            .call(
                host,
                false,
                "POST",
                &format!("/suggestions/{id}/feedback"),
                Some(json!({"action": action})),
            )
            .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }
    let uri = format!("/admin/suggestions/export?label=wrong&{}", range());
    let (st, _) = h.call(host, false, "GET", &uri, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = h
        .call(
            admin,
            true,
            "GET",
            &format!("/admin/suggestions/export?label=bogus&{}", range()),
            None,
        )
        .await;
    assert!(st.is_client_error());

    let (st, v) = h.call(admin, true, "GET", &uri, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let meetings: Vec<eval_mirror::Meeting> = v["meetings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| serde_json::from_value(m.clone()).unwrap())
        .collect();
    let m = meetings
        .iter()
        .find(|m| m.id == format!("fb-{bad}"))
        .unwrap();
    assert!(
        meetings
            .iter()
            .all(|m| m.id != format!("fb-{meh}") && m.id != format!("fb-{fine}"))
    );
    eval_mirror::validate(m);
    assert_eq!(m.language, "en");
    assert!(m.title.contains("wrong") && m.snippets.is_empty());
    assert_eq!(m.participants, vec!["Speaker 1", "Speaker 2"]);
    // Ids and PII are gone; duplicate texts were disambiguated.
    let raw = serde_json::to_string(&v["meetings"]).unwrap();
    assert!(
        !raw.contains("alice@corp.com")
            && !raw.contains("Alice")
            && !raw.contains(&h.ws.to_string())
            && !raw.contains(&h.room.to_string())
    );
    assert_eq!(m.lines.len(), 3);
    assert_eq!(m.lines[2].t_ms, 1000, "time never goes back");
    let exp = &m.expectations.suggestions;
    assert_eq!(
        (exp.forbidden[0].at_line, exp.forbidden[0].category.as_str()),
        (2, "fact")
    );
    assert_eq!(exp.must_not_include.len(), 1);

    // The other label.
    let uri = format!("/admin/suggestions/export?label=not_helpful&{}", range());
    let (_, v) = h.call(admin, true, "GET", &uri, None).await;
    let ids: Vec<&str> = v["meetings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&format!("fb-{meh}").as_str()));
}
