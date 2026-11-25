#![allow(clippy::unwrap_used)]
//! Post-session summary: service, worker handlers and REST against real Postgres/Redis with a
//! fake LLM. Skips when DATABASE_URL / REDIS_URL are unset.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use chrono::Utc;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_assistant::http::summary::{SummaryApi, router};
use warptalk_assistant::suggestion::{
    DataBoundary, LlmResolver, Purpose, ResolveError, ResolvedLlm,
};
use warptalk_assistant::summary::{
    NoBus, NoFollowUps, PgSummaryRepository, RoomAccess, RoomAccessInfo, RoomMeta, RoomMetaSource,
    RunOutcome, SummaryDeps, SummaryService, SummaryStatus, SummaryWorker, TranscriptData,
    TranscriptSegment, TranscriptSource, WorkspaceScope, register_config,
};
use warptalk_bus::StreamMessage;
use warptalk_config::JwtConfig;
use warptalk_contracts::events::SttResult;
use warptalk_kernel::{AppResult, RoomId, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, Usage,
};
use warptalk_settings::{Scope, SettingsStore, register_defaults};
use warptalk_web::JwtKeys;

// ---------------------------------------------------------------- fakes

type Responder = Arc<dyn Fn(&Request) -> Result<Value, LlmError> + Send + Sync>;

struct FakeLlm {
    responder: Mutex<Responder>,
    calls: Mutex<Vec<Request>>,
    delay: Mutex<Duration>,
}

impl FakeLlm {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            responder: Mutex::new(Arc::new(default_responder)),
            calls: Mutex::new(Vec::new()),
            delay: Mutex::new(Duration::ZERO),
        })
    }
    fn set(&self, r: impl Fn(&Request) -> Result<Value, LlmError> + Send + Sync + 'static) {
        *self.responder.lock().unwrap() = Arc::new(r);
    }
    fn stage_count(&self, stage: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.metadata.get("stage").map(String::as_str) == Some(stage))
            .count()
    }
    fn total(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

fn user_text(req: &Request) -> String {
    req.messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn first_ref(text: &str) -> String {
    let i = text.find("[s").unwrap();
    let rest = &text[i + 1..];
    rest[..rest.find(']').unwrap()].to_owned()
}

/// map/single: one real item (Bob / "by Friday") and one hallucinated (Zed, bogus ref);
/// reduce: merges the partial records it was given.
fn default_responder(req: &Request) -> Result<Value, LlmError> {
    let text = user_text(req);
    let stage = req.metadata.get("stage").cloned().unwrap_or_default();
    if stage == "reduce" {
        // Each partial record sits in its own `<<DATA-n:partial_i>> .. <<END-n:partial_i>>` fence.
        let tail = text.split("Partial records:\n").nth(1).unwrap();
        let mut parts: Vec<Value> = Vec::new();
        for chunk in tail.split("<<DATA-").skip(1) {
            let body = chunk.split_once(">>\n").unwrap().1;
            let body = body.split("\n<<END-").next().unwrap();
            parts.extend(serde_json::from_str::<Vec<Value>>(body).unwrap());
        }
        let items: Vec<Value> = parts
            .iter()
            .flat_map(|p| p["action_items"].as_array().unwrap().clone())
            .collect();
        return Ok(json!({
            "summary_short": "merged short", "summary_detailed": "merged detailed ".repeat(20),
            "decisions": ["Ship on Monday"], "action_items": items,
            "open_questions": [], "topics": ["budget"]
        }));
    }
    let r = first_ref(&text);
    Ok(json!({
        "summary_short": format!("short {stage}"),
        "summary_detailed": "d".repeat(if stage == "map" { 1800 } else { 50 }),
        "decisions": ["Ship on Monday", "ship on monday"],
        "action_items": [
            {"task": format!("Send budget ({r})"), "owner": "Bob", "due": "by Friday", "source_segments": [r]},
            {"task": format!("Book a room ({r})"), "owner": "Zed", "due": "next year", "source_segments": ["s99999"]}
        ],
        "open_questions": ["Who pays?"], "topics": ["budget"]
    }))
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _: &str) -> Capabilities {
        Capabilities::conservative()
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        self.calls.lock().unwrap().push(req.clone());
        let delay = *self.delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let responder = self.responder.lock().unwrap().clone();
        responder(req).map(|v| Response {
            parts: vec![Part::Text(v.to_string())],
            stop_reason: StopReason::End,
            usage: Usage {
                input_uncached: 100,
                output: 20,
                ..Default::default()
            },
            provider_request_id: None,
        })
    }
    async fn stream(
        &self,
        _: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest("no streaming".into()))
    }
    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct FakeResolver(Mutex<Option<Arc<FakeLlm>>>, Mutex<DataBoundary>);

struct FakePolicy(std::sync::atomic::AtomicBool);

#[async_trait]
impl warptalk_assistant::summary::ExternalLlmGate for FakePolicy {
    async fn external_llm_allowed(&self, _: WorkspaceId) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmResolver for FakeResolver {
    async fn for_purpose(&self, _: WorkspaceId, p: Purpose) -> Result<ResolvedLlm, ResolveError> {
        assert_eq!(p, Purpose::Summary);
        match self.0.lock().unwrap().clone() {
            Some(l) => {
                let boundary = *self.1.lock().unwrap();
                Ok(ResolvedLlm {
                    provider: l,
                    model: Some("fake-model".into()),
                    boundary,
                    key_source: boundary.into(),
                })
            }
            None => Err(ResolveError::NotConfigured),
        }
    }
}

struct MemTranscripts(Mutex<Option<TranscriptData>>);

#[async_trait]
impl TranscriptSource for MemTranscripts {
    async fn load(&self, _: RoomId, limit: u32) -> AppResult<Option<TranscriptData>> {
        Ok(self.0.lock().unwrap().clone().map(|mut t| {
            t.segments.truncate(limit as usize);
            t
        }))
    }
}

struct MemRooms(WorkspaceId);

#[async_trait]
impl RoomMetaSource for MemRooms {
    async fn room(&self, _: RoomId) -> AppResult<Option<RoomMeta>> {
        Ok(Some(RoomMeta {
            workspace_id: self.0,
            title: "Budget sync".into(),
            source_language: "en".into(),
            target_languages: vec!["vi".into()],
            participants: vec!["Ann".into(), "Bob".into()],
        }))
    }
}

#[derive(Default)]
struct FakeAccess {
    rooms: Mutex<HashMap<(RoomId, UserId), RoomAccessInfo>>,
    scopes: Mutex<HashMap<(WorkspaceId, UserId), WorkspaceScope>>,
}

#[async_trait]
impl RoomAccess for FakeAccess {
    async fn room_access(&self, room: RoomId, user: UserId) -> AppResult<Option<RoomAccessInfo>> {
        Ok(self.rooms.lock().unwrap().get(&(room, user)).cloned())
    }
    async fn workspace_scope(
        &self,
        ws: WorkspaceId,
        user: UserId,
    ) -> AppResult<Option<WorkspaceScope>> {
        Ok(self.scopes.lock().unwrap().get(&(ws, user)).cloned())
    }
}

// ---------------------------------------------------------------- harness

struct H {
    svc: Arc<SummaryService>,
    worker: Arc<SummaryWorker>,
    llm: Arc<FakeLlm>,
    resolver: Arc<FakeResolver>,
    policy: Arc<FakePolicy>,
    tr: Arc<MemTranscripts>,
    access: Arc<FakeAccess>,
    store: SettingsStore,
    pool: PgPool,
    jwt: JwtKeys,
    ws: WorkspaceId,
    room: RoomId,
    ids: Vec<Uuid>,
}

fn segments(n: usize, chars: usize) -> Vec<TranscriptSegment> {
    (0..n)
        .map(|i| TranscriptSegment {
            id: Uuid::now_v7(),
            speaker: if i % 2 == 0 {
                "Ann".into()
            } else {
                "Bob".into()
            },
            text: if i == 0 {
                "Bob will send the budget by Friday.".into()
            } else {
                format!("point {i} {}", "word ".repeat(chars / 5))
            },
            language: "en".into(),
            start_ms: (i * 1000) as i64,
        })
        .collect()
}

const FAST: &str = r#""backoff_base_ms": 0, "backoff_max_ms": 0, "transcript_wait_secs": 0, "transcript_poll_ms": 10"#;

async fn harness(n_segments: usize, seg_chars: usize, extra_cfg: &str) -> Option<H> {
    let (Ok(db), Ok(_redis)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL")) else {
        println!("skipping: DATABASE_URL / REDIS_URL not set");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_settings::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let store = SettingsStore::new(pool.clone(), None);
    register_defaults(&store);
    register_config(&store);
    let (ws, room) = (WorkspaceId::new(), RoomId::new());
    let doc: Value = serde_json::from_str(&format!("{{{FAST}{extra_cfg}}}")).unwrap();
    store
        .put(
            Scope::Workspace(ws),
            warptalk_assistant::summary::NS_SUMMARY,
            doc,
            None,
        )
        .await
        .unwrap();
    let llm = FakeLlm::new();
    let resolver = Arc::new(FakeResolver(
        Mutex::new(Some(llm.clone())),
        Mutex::new(DataBoundary::External),
    ));
    let segs = segments(n_segments, seg_chars);
    let ids = segs.iter().map(|s| s.id).collect();
    let tr = Arc::new(MemTranscripts(Mutex::new(Some(TranscriptData {
        workspace_id: ws,
        version: 1,
        finalized: true,
        total_segments: n_segments as u32,
        segments: segs,
    }))));
    let policy = Arc::new(FakePolicy(true.into()));
    let access = Arc::new(FakeAccess::default());
    let svc = Arc::new(SummaryService::new(SummaryDeps {
        repo: PgSummaryRepository::new(pool.clone()),
        transcripts: tr.clone(),
        rooms: Arc::new(MemRooms(ws)),
        followups: Arc::new(NoFollowUps),
        llm: resolver.clone(),
        policy: policy.clone(),
        settings: store.clone(),
        bus: Arc::new(NoBus),
        clock: Arc::new(SystemClock),
    }));
    let worker = Arc::new(SummaryWorker::new(svc.clone(), 4));
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    Some(H {
        svc,
        worker,
        llm,
        resolver,
        policy,
        tr,
        access,
        store,
        pool,
        jwt,
        ws,
        room,
        ids,
    })
}

impl H {
    fn marker(&self) -> StreamMessage {
        let m = SttResult::meeting_end(Uuid::now_v7().to_string(), self.room, 1);
        StreamMessage {
            stream: "stt:results".into(),
            id: "1-0".into(),
            fields: HashMap::from([("data".to_owned(), serde_json::to_string(&m).unwrap())]),
        }
    }

    /// Feed the marker through the real handler and wait for the spawned job.
    async fn end_meeting(&self) {
        let handlers = self.worker.consumers();
        handlers[0].1.handle(&self.marker()).await.unwrap();
        self.worker.drain().await;
    }

    async fn rows(&self) -> Vec<(i32, String, Option<String>)> {
        sqlx::query_as(
            "SELECT version, status, error_code FROM assistant.meeting_summaries \
             WHERE room_id = $1 ORDER BY version",
        )
        .bind(self.room.as_uuid())
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    fn app(&self) -> Router {
        router(
            SummaryApi {
                service: self.svc.clone(),
                access: self.access.clone(),
                clock: Arc::new(SystemClock),
            },
            self.jwt.clone(),
        )
    }

    fn token(&self, user: UserId) -> String {
        self.jwt
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
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
    }

    async fn call(
        &self,
        user: UserId,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token(user)));
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
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn marker_triggers_exactly_one_job_and_validates_owner_and_due() {
    let Some(h) = harness(6, 40, "").await else {
        return;
    };
    h.end_meeting().await;
    // The same marker redelivered (and a second one) must not enqueue another job.
    h.end_meeting().await;
    h.end_meeting().await;
    assert_eq!(h.rows().await, vec![(1, "completed".into(), None)]);
    assert_eq!(h.llm.total(), 1, "one single-pass call");

    let job = h.svc.repo().latest(h.room).await.unwrap().unwrap();
    let c = job.content.unwrap();
    assert_eq!(job.model.as_deref(), Some("fake-model"));
    assert_eq!(job.prompt_version.as_deref(), Some("v1"));
    assert_eq!(c.language, "en");
    assert_eq!(c.decisions, vec!["Ship on Monday"], "de-duplicated");
    assert_eq!(c.meta.windows, 1);
    assert_eq!(c.action_items.len(), 2);
    let real = &c.action_items[0];
    assert_eq!(real.owner.as_deref(), Some("Bob"));
    assert_eq!(real.due.as_deref(), Some("by Friday"));
    assert_eq!(real.source_segment_ids, vec![h.ids[0]]);
    let ghost = &c.action_items[1];
    assert_eq!((ghost.owner.as_deref(), ghost.due.as_deref()), (None, None));
    assert!(ghost.source_segment_ids.is_empty(), "unknown refs dropped");

    let items = h.svc.repo().items_for_room(h.ws, h.room).await.unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.status == "open"));
    assert_eq!(items[1].owner, None);
}

#[tokio::test]
async fn prompts_are_overridable_through_settings() {
    let Some(h) = harness(
        4,
        40,
        r#", "language": "vi", "prompt_version": "v7", "system_prompt": "CUSTOM SYSTEM in {{language}}", "model": "m""#,
    )
    .await
    else {
        return;
    };
    h.end_meeting().await;
    let calls = h.llm.calls.lock().unwrap().clone();
    // The override is followed by the per-call untrusted-data rule.
    assert!(
        calls[0]
            .system
            .as_deref()
            .unwrap()
            .starts_with("CUSTOM SYSTEM in vi\n\n")
    );
    assert_eq!(
        calls[0].metadata.get("purpose").map(String::as_str),
        Some("summary")
    );
    assert!(calls[0].response_format.as_ref().unwrap().strict);
    let job = h.svc.repo().latest(h.room).await.unwrap().unwrap();
    assert_eq!(job.prompt_version.as_deref(), Some("v7"));
    assert_eq!(job.content.unwrap().language, "vi");
}

#[tokio::test]
async fn long_transcripts_are_map_reduced_over_windows() {
    // 300 segments x ~400 chars ~ 30k tokens; 2000-token windows.
    let Some(h) = harness(300, 400, r#", "window_tokens": 2000"#).await else {
        return;
    };
    h.end_meeting().await;
    assert_eq!(h.rows().await, vec![(1, "completed".into(), None)]);
    let maps = h.llm.stage_count("map");
    assert!(maps >= 10, "expected many windows, got {maps}");
    assert!(
        h.llm.stage_count("reduce") >= 2,
        "hierarchical reduce when partials do not fit"
    );
    assert_eq!(h.llm.stage_count("single"), 0);
    let c = h
        .svc
        .repo()
        .latest(h.room)
        .await
        .unwrap()
        .unwrap()
        .content
        .unwrap();
    assert_eq!(c.meta.windows as usize, maps);
    assert_eq!(c.meta.input_segments, 300);
    // Refs from windows far into the meeting resolve to real segment ids.
    assert!(c.action_items.iter().any(|a| {
        a.source_segment_ids
            .first()
            .is_some_and(|id| h.ids.iter().position(|x| x == id).unwrap() > 100)
    }));
    assert_eq!(c.summary_short, "merged short");
}

#[tokio::test]
async fn segment_cap_is_applied_and_flagged() {
    let Some(h) = harness(25, 40, r#", "max_segments": 10"#).await else {
        return;
    };
    h.end_meeting().await;
    let c = h
        .svc
        .repo()
        .latest(h.room)
        .await
        .unwrap()
        .unwrap()
        .content
        .unwrap();
    assert_eq!(
        (
            c.meta.input_segments,
            c.meta.total_segments,
            c.meta.truncated
        ),
        (10, 25, true)
    );
}

#[tokio::test]
async fn regenerate_creates_version_2_and_keeps_done_items() {
    let Some(h) = harness(6, 40, "").await else {
        return;
    };
    let (host, other) = (UserId::new(), UserId::new());
    h.grant(host, true, true, None);
    h.grant(other, false, true, None);
    h.end_meeting().await;

    // Mark the real item done through REST.
    let (st, items) = h
        .call(
            host,
            "GET",
            &format!("/meetings/{}/action-items", h.room.as_uuid()),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let item_id = items["items"][0]["id"].as_str().unwrap().to_owned();
    let (st, v) = h
        .call(
            host,
            "PATCH",
            &format!("/meetings/{}/action-items/{item_id}", h.room.as_uuid()),
            Some(json!({"status": "done"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "done");

    let uri = format!("/meetings/{}/summary/regenerate", h.room.as_uuid());
    let (st, _) = h.call(other, "POST", &uri, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "participants cannot regenerate");
    let (st, v) = h.call(host, "POST", &uri, None).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["version"], 2);
    assert_eq!(v["status"], "pending");
    let (st, v) = h.call(host, "POST", &uri, None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "SUMMARY_IN_PROGRESS");

    // GET while pending shows the pending latest and the previous completed version.
    let (_, v) = h
        .call(
            host,
            "GET",
            &format!("/meetings/{}/summary", h.room.as_uuid()),
            None,
        )
        .await;
    assert_eq!(v["status"], "pending");
    assert_eq!(v["lastCompleted"]["version"], 1);

    // The sweeper (quiet = 0) runs it.
    assert_eq!(h.worker.sweep(Duration::ZERO).await, 1);
    h.worker.drain().await;
    assert_eq!(
        h.rows().await,
        vec![(1, "completed".into(), None), (2, "completed".into(), None)]
    );
    let (_, v) = h
        .call(
            host,
            "GET",
            &format!("/meetings/{}/summary", h.room.as_uuid()),
            None,
        )
        .await;
    assert_eq!(
        (v["status"].as_str(), v["latest"]["version"].as_i64()),
        (Some("completed"), Some(2))
    );
    let items = h.svc.repo().items_for_room(h.ws, h.room).await.unwrap();
    let done: Vec<_> = items.iter().filter(|i| i.status == "done").collect();
    assert_eq!(done.len(), 1, "done state carried over to the new version");
    assert!(done[0].owner.as_deref() == Some("Bob"));
}

#[tokio::test]
async fn rest_rbac_and_action_item_edits() {
    let Some(h) = harness(6, 40, "").await else {
        return;
    };
    let (host, participant, admin, outsider, stranger) = (
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
    );
    h.grant(host, true, true, Some(WorkspaceRole::Member));
    h.grant(participant, false, true, Some(WorkspaceRole::Member));
    h.grant(admin, false, false, Some(WorkspaceRole::Admin));
    h.grant(outsider, false, false, Some(WorkspaceRole::Member));
    h.end_meeting().await;
    let base = format!("/meetings/{}", h.room.as_uuid());

    for (u, want) in [
        (host, 200),
        (participant, 200),
        (admin, 200),
        (outsider, 403),
        (stranger, 404),
    ] {
        let (st, _) = h.call(u, "GET", &format!("{base}/summary"), None).await;
        assert_eq!(st.as_u16(), want, "summary for {u:?}");
        let (st, _) = h
            .call(u, "GET", &format!("{base}/action-items"), None)
            .await;
        assert_eq!(st.as_u16(), want, "items for {u:?}");
    }
    // No token: 401.
    let resp = h
        .app()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("{base}/summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Regenerate: host and admin yes, participant and outsider no.
    let (st, _) = h
        .call(
            outsider,
            "POST",
            &format!("{base}/summary/regenerate"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (_, items) = h
        .call(participant, "GET", &format!("{base}/action-items"), None)
        .await;
    let id = items["items"][0]["id"].as_str().unwrap().to_owned();
    let patch = format!("{base}/action-items/{id}");
    let (st, v) = h
        .call(
            participant,
            "PATCH",
            &patch,
            Some(json!({"task": "  Send budget v2 ", "owner": "Ann", "due": null})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["task"].as_str(), v["owner"].as_str(), v["due"].is_null()),
        (Some("Send budget v2"), Some("Ann"), true)
    );
    let (st, _) = h
        .call(
            participant,
            "PATCH",
            &patch,
            Some(json!({"status": "weird"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = h
        .call(participant, "PATCH", &patch, Some(json!({"task": "  "})))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = h
        .call(outsider, "PATCH", &patch, Some(json!({"status": "done"})))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // An item of another room is not reachable through this room.
    let other_room = RoomId::new();
    h.access.rooms.lock().unwrap().insert(
        (other_room, host),
        RoomAccessInfo {
            workspace_id: h.ws,
            is_host: true,
            is_participant: true,
            workspace_role: None,
        },
    );
    let (st, _) = h
        .call(
            host,
            "PATCH",
            &format!("/meetings/{}/action-items/{id}", other_room.as_uuid()),
            Some(json!({"status": "done"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Workspace list: admin sees everything, a member only the rooms they are in; `mine` filters
    // by owner name; a non-member is refused.
    h.access.scopes.lock().unwrap().insert(
        (h.ws, admin),
        WorkspaceScope {
            role: WorkspaceRole::Admin,
            room_ids: vec![],
            names: vec!["Ann".into()],
        },
    );
    h.access.scopes.lock().unwrap().insert(
        (h.ws, participant),
        WorkspaceScope {
            role: WorkspaceRole::Member,
            room_ids: vec![h.room],
            names: vec!["Ann".into()],
        },
    );
    h.access.scopes.lock().unwrap().insert(
        (h.ws, outsider),
        WorkspaceScope {
            role: WorkspaceRole::Member,
            room_ids: vec![],
            names: vec!["Ann".into()],
        },
    );
    let list = format!("/workspaces/{}/action-items", h.ws.as_uuid());
    let (st, v) = h
        .call(admin, "GET", &format!("{list}?status=open"), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = h
        .call(
            participant,
            "GET",
            &format!("{list}?status=open&mine=true"),
            None,
        )
        .await;
    let mine = v["items"].as_array().unwrap();
    assert_eq!(mine.len(), 1, "{v}");
    assert_eq!(mine[0]["owner"], "Ann");
    let (_, v) = h
        .call(participant, "GET", &format!("{list}?status=done"), None)
        .await;
    assert!(v["items"].as_array().unwrap().is_empty());
    let (_, v) = h.call(outsider, "GET", &list, None).await;
    assert!(
        v["items"].as_array().unwrap().is_empty(),
        "member outside the room sees nothing"
    );
    let (st, _) = h.call(stranger, "GET", &list, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = h
        .call(admin, "GET", &format!("{list}?status=bogus"), None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_an_llm_the_job_fails_with_llm_not_configured() {
    let Some(h) = harness(4, 40, "").await else {
        return;
    };
    *h.resolver.0.lock().unwrap() = None;
    h.end_meeting().await;
    assert_eq!(
        h.rows().await,
        vec![(1, "failed".into(), Some("LLM_NOT_CONFIGURED".into()))]
    );
    let host = UserId::new();
    h.grant(host, true, true, None);
    let (_, v) = h
        .call(
            host,
            "GET",
            &format!("/meetings/{}/summary", h.room.as_uuid()),
            None,
        )
        .await;
    assert_eq!(v["status"], "failed");
    assert_eq!(v["latest"]["errorCode"], "LLM_NOT_CONFIGURED");
}

#[tokio::test]
async fn transient_llm_errors_retry_then_fail_and_bad_output_is_permanent() {
    let Some(h) = harness(4, 40, r#", "max_attempts": 2, "max_job_attempts": 2"#).await else {
        return;
    };
    h.llm.set(|_| Err(LlmError::Overloaded("busy".into())));
    h.end_meeting().await;
    // Attempt 1 exhausted its call retries (2 calls) and went back to pending.
    assert_eq!(h.rows().await, vec![(1, "pending".into(), None)]);
    assert_eq!(h.llm.total(), 2);
    h.worker.sweep(Duration::ZERO).await;
    h.worker.drain().await;
    assert_eq!(
        h.rows().await,
        vec![(1, "failed".into(), Some("LLM_UNAVAILABLE".into()))]
    );
    assert_eq!(h.llm.total(), 4);

    // The provider recovers: regenerate works and succeeds.
    h.llm.set(default_responder);
    let job = h.svc.regenerate(h.ws, h.room, UserId::new()).await.unwrap();
    assert_eq!(h.svc.run(job.id).await, RunOutcome::Completed);

    // A rejected key fails permanently (no job-level retry).
    h.llm.set(|_| Err(LlmError::Auth("bad key".into())));
    let job = h.svc.regenerate(h.ws, h.room, UserId::new()).await.unwrap();
    assert_eq!(
        h.svc.run(job.id).await,
        RunOutcome::Failed("LLM_FAILED".into())
    );
    // Unparseable output is permanent too.
    h.llm.set(|_| Ok(json!("not an object")));
    let job = h.svc.regenerate(h.ws, h.room, UserId::new()).await.unwrap();
    assert_eq!(
        h.svc.run(job.id).await,
        RunOutcome::Failed("LLM_BAD_OUTPUT".into())
    );
    let job = h.svc.regenerate(h.ws, h.room, UserId::new()).await.unwrap();
    assert_eq!(job.version, 5);
    // A job is claimed once: running it again is a no-op.
    h.llm.set(default_responder);
    assert_eq!(h.svc.run(job.id).await, RunOutcome::Completed);
    assert_eq!(h.svc.run(job.id).await, RunOutcome::Skipped);
    let latest = h.svc.repo().latest(h.room).await.unwrap().unwrap();
    assert_eq!(latest.status, SummaryStatus::Completed);
}

#[tokio::test]
async fn empty_transcript_fails_with_no_transcript() {
    let Some(h) = harness(3, 40, "").await else {
        return;
    };
    *h.tr.0.lock().unwrap() = Some(TranscriptData {
        workspace_id: h.ws,
        version: 1,
        finalized: true,
        segments: vec![],
        total_segments: 0,
    });
    h.end_meeting().await;
    assert_eq!(
        h.rows().await,
        vec![(1, "failed".into(), Some("NO_TRANSCRIPT".into()))]
    );
    // And no transcript at all: the marker enqueues nothing.
    *h.tr.0.lock().unwrap() = None;
    let h2_room = RoomId::new();
    let handlers = h.worker.consumers();
    let m = SttResult::meeting_end("x".into(), h2_room, 1);
    let msg = StreamMessage {
        stream: "stt:results".into(),
        id: "2-0".into(),
        fields: HashMap::from([("data".to_owned(), serde_json::to_string(&m).unwrap())]),
    };
    handlers[0].1.handle(&msg).await.unwrap();
    h.worker.drain().await;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM assistant.meeting_summaries WHERE room_id = $1")
            .bind(h2_room.as_uuid())
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn regular_segments_and_disabled_config_enqueue_nothing() {
    let Some(h) = harness(4, 40, r#", "enabled": false"#).await else {
        return;
    };
    let handlers = h.worker.consumers();
    let mut seg = SttResult::meeting_end("s".into(), h.room, 1);
    seg.text = "hello world".into();
    let msg = StreamMessage {
        stream: "stt:results".into(),
        id: "3-0".into(),
        fields: HashMap::from([("data".to_owned(), serde_json::to_string(&seg).unwrap())]),
    };
    handlers[0].1.handle(&msg).await.unwrap();
    h.end_meeting().await;
    assert!(h.rows().await.is_empty());
    assert_eq!(h.llm.total(), 0);
    let _ = &h.store;
}

/// NFR-PF-07: a 1-hour meeting (~900 segments, ~13k tokens) completes quickly. The fake LLM
/// takes 400 ms per call; real calls take seconds each, so the figure to compare is the number
/// of sequential LLM rounds (printed).
#[tokio::test]
async fn one_hour_meeting_latency_with_a_fake_llm() {
    let Some(h) = harness(900, 60, "").await else {
        return;
    };
    *h.llm.delay.lock().unwrap() = Duration::from_millis(400);
    let t = Instant::now();
    h.end_meeting().await;
    let elapsed = t.elapsed();
    let c = h
        .svc
        .repo()
        .latest(h.room)
        .await
        .unwrap()
        .unwrap()
        .content
        .unwrap();
    println!(
        "one-hour meeting: {} segments, {} windows, {} llm calls, wall {:?}",
        c.meta.input_segments, c.meta.windows, c.meta.llm_calls, elapsed
    );
    assert_eq!(h.rows().await, vec![(1, "completed".into(), None)]);
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

fn set_policy(h: &H, allowed: bool) {
    h.policy
        .0
        .store(allowed, std::sync::atomic::Ordering::SeqCst);
}

#[tokio::test]
async fn disallowed_external_llm_fails_with_code_and_no_llm_call() {
    let Some(h) = harness(4, 40, "").await else {
        return;
    };
    set_policy(&h, false);
    h.end_meeting().await;
    assert_eq!(
        h.rows().await,
        vec![(1, "failed".into(), Some("EXTERNAL_LLM_NOT_ALLOWED".into()))]
    );
    assert_eq!(h.llm.total(), 0);
    let host = UserId::new();
    h.grant(host, true, true, None);
    let path = format!("/meetings/{}/summary", h.room.as_uuid());
    let (_, v) = h.call(host, "GET", &path, None).await;
    assert_eq!(v["status"], "failed");
    assert_eq!(v["latest"]["errorCode"], "EXTERNAL_LLM_NOT_ALLOWED");
    // Regenerate is refused up front: 400, no new row, still no LLM call.
    let (st, v) = h
        .call(host, "POST", &format!("{path}/regenerate"), None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "EXTERNAL_LLM_NOT_ALLOWED");
    assert_eq!(h.rows().await.len(), 1);
    assert_eq!(h.llm.total(), 0);
}

#[tokio::test]
async fn disallowed_but_workspace_controlled_llm_completes() {
    let Some(h) = harness(4, 40, "").await else {
        return;
    };
    set_policy(&h, false);
    *h.resolver.1.lock().unwrap() = DataBoundary::WorkspaceControlled;
    h.end_meeting().await;
    assert_eq!(h.rows().await, vec![(1, "completed".into(), None)]);
    assert!(h.llm.total() > 0);
}

#[tokio::test]
async fn allowed_external_llm_completes() {
    let Some(h) = harness(4, 40, "").await else {
        return;
    };
    h.end_meeting().await;
    assert_eq!(h.rows().await, vec![(1, "completed".into(), None)]);
}
