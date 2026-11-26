#![allow(clippy::unwrap_used)]
//! HTTP integration tests for WarpBot chat. They need Postgres and skip when DATABASE_URL is
//! unset. The LLM, knowledge index and membership are fakes; storage and routes are real.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request as HttpRequest, StatusCode};
use chrono::Utc;
use futures::StreamExt;
use futures::stream::BoxStream;
use secrecy::SecretString;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_assistant::application::chat_ports::{
    AssistantLlmResolver, KnowledgeHit, WorkspaceKnowledge, WorkspaceLlmConsent,
};
use warptalk_assistant::application::ports::{
    KeyCheckError, KeyValidator, NoopNotifier, SecretVault, VaultError, WorkspaceAccess,
};
use warptalk_assistant::domain::byo::{LlmProviderKind, SealedSecret};
use warptalk_assistant::infrastructure::InMemoryChatEventBus;
use warptalk_assistant::suggestion::{DataBoundary, ResolveError, ResolvedLlm};
use warptalk_assistant::{AssistantDeps, AssistantModule, ChatDeps, MIGRATIONS};
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, ToolChoice,
};
use warptalk_web::JwtKeys;

// ---------------------------------------------------------------- fakes

/// One scripted model round: events with a delay before each.
type Round = Vec<(Duration, StreamEvent)>;

#[derive(Default)]
struct ScriptedLlm {
    rounds: Mutex<VecDeque<Round>>,
    requests: Mutex<Vec<Request>>,
}

impl ScriptedLlm {
    fn push(&self, round: Round) {
        self.rounds.lock().unwrap().push_back(round);
    }
}

fn text(t: &str) -> (Duration, StreamEvent) {
    (Duration::ZERO, StreamEvent::TextDelta(t.to_owned()))
}

fn done() -> (Duration, StreamEvent) {
    (
        Duration::ZERO,
        StreamEvent::Done {
            stop_reason: StopReason::End,
        },
    )
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _: &str) -> Capabilities {
        Capabilities {
            tools: true,
            ..Capabilities::conservative()
        }
    }
    async fn complete(&self, _: &Request) -> Result<Response, LlmError> {
        Err(LlmError::InvalidRequest("chat streams".into()))
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        self.requests.lock().unwrap().push(req.clone());
        let round = self
            .rounds
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| LlmError::InvalidRequest("script exhausted".into()))?;
        Ok(futures::stream::iter(round)
            .then(|(delay, ev)| async move {
                tokio::time::sleep(delay).await;
                Ok(ev)
            })
            .boxed())
    }
    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct FixedResolver(Arc<ScriptedLlm>);

#[async_trait]
impl AssistantLlmResolver for FixedResolver {
    async fn resolve(&self, _: WorkspaceId) -> Result<ResolvedLlm, ResolveError> {
        Ok(ResolvedLlm {
            provider: self.0.clone(),
            model: Some("test-model".into()),
            boundary: DataBoundary::External,
        })
    }
}

struct Consent(bool);

#[async_trait]
impl WorkspaceLlmConsent for Consent {
    async fn external_llm_allowed(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(self.0)
    }
}

#[derive(Default)]
struct FakeKnowledge {
    queries: Mutex<Vec<String>>,
}

#[async_trait]
impl WorkspaceKnowledge for FakeKnowledge {
    async fn search(
        &self,
        _: WorkspaceId,
        _: UserId,
        query: &str,
        _: usize,
    ) -> AppResult<Vec<KnowledgeHit>> {
        self.queries.lock().unwrap().push(query.to_owned());
        Ok(vec![KnowledgeHit {
            title: "Roadmap 2027".into(),
            text: "Launch the mobile app in Q2.".into(),
            kind: "document".into(),
            ref_id: Some("doc-1".into()),
            score: 0.9,
        }])
    }
}

#[derive(Default)]
struct Members(Mutex<HashSet<(Uuid, Uuid)>>);

#[async_trait]
impl WorkspaceAccess for Members {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        let ok = self
            .0
            .lock()
            .unwrap()
            .contains(&(ws.as_uuid(), user.as_uuid()));
        Ok(ok.then_some(WorkspaceRole::Member))
    }
}

struct NoVault;

impl SecretVault for NoVault {
    fn is_enabled(&self) -> bool {
        false
    }
    fn seal(
        &self,
        _: WorkspaceId,
        _: LlmProviderKind,
        _: &SecretString,
    ) -> Result<SealedSecret, VaultError> {
        Err(VaultError::Disabled("test".into()))
    }
    fn open(
        &self,
        _: WorkspaceId,
        _: LlmProviderKind,
        _: &SealedSecret,
    ) -> Result<SecretString, VaultError> {
        Err(VaultError::Disabled("test".into()))
    }
}

struct NoValidator;

#[async_trait]
impl KeyValidator for NoValidator {
    async fn check_base_url(&self, _: &str) -> Result<(), KeyCheckError> {
        Ok(())
    }
    async fn validate(
        &self,
        _: LlmProviderKind,
        _: Option<&str>,
        _: &SecretString,
    ) -> Result<(), KeyCheckError> {
        Ok(())
    }
}

// ---------------------------------------------------------------- harness

struct Ctx {
    router: axum::Router,
    keys: JwtKeys,
    llm: Arc<ScriptedLlm>,
    knowledge: Arc<FakeKnowledge>,
    ws: Uuid,
    user: UserId,
}

async fn ctx() -> Option<Ctx> {
    ctx_with(true).await
}

async fn ctx_with(consent: bool) -> Option<Ctx> {
    let Ok(db) = std::env::var("DATABASE_URL") else {
        println!("DATABASE_URL unset; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let ws = Uuid::now_v7();
    let user = UserId::from_uuid(Uuid::now_v7());
    let members = Arc::new(Members::default());
    members.0.lock().unwrap().insert((ws, user.as_uuid()));
    let llm = Arc::new(ScriptedLlm::default());
    let knowledge = Arc::new(FakeKnowledge::default());
    let mut module = AssistantModule::new(AssistantDeps {
        pool,
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        access: members,
        vault: Arc::new(NoVault),
        validator: Arc::new(NoValidator),
        notifier: Arc::new(NoopNotifier),
    });
    module.enable_chat(ChatDeps {
        bus: Arc::new(InMemoryChatEventBus::new()),
        llm: Arc::new(FixedResolver(llm.clone())),
        consent: Arc::new(Consent(consent)),
        knowledge: Some(knowledge.clone()),
        default_model: None,
    });
    Some(Ctx {
        router: module.router(),
        keys,
        llm,
        knowledge,
        ws,
        user,
    })
}

impl Ctx {
    fn token(&self, user: UserId) -> String {
        self.keys
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
            .unwrap()
            .0
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        user: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token(user)));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn create(&self) -> Uuid {
        let (s, v) = self
            .call(
                Method::POST,
                "/assistant/conversations",
                self.user,
                Some(json!({ "workspaceId": self.ws })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(v["title"], "New chat");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn send(&self, conv: Uuid, content: &str) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            &format!("/assistant/conversations/{conv}/messages"),
            self.user,
            Some(json!({
                "content": content,
                "pageContext": { "pageType": "documents", "snapshot": { "count": 3 } }
            })),
        )
        .await
    }

    /// Polls the detail until the reply leaves `streaming`.
    async fn wait_reply(&self, conv: Uuid, reply: &str) -> Value {
        for _ in 0..100 {
            let (s, v) = self
                .call(
                    Method::GET,
                    &format!("/assistant/conversations/{conv}"),
                    self.user,
                    None,
                )
                .await;
            assert_eq!(s, StatusCode::OK, "{v}");
            let m = v["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == reply)
                .cloned()
                .unwrap();
            if m["status"] != "streaming" {
                return m;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("reply never finished");
    }
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn answers_with_search_and_cited_sources() {
    let Some(c) = ctx().await else { return };
    c.llm.push(vec![
        (
            Duration::ZERO,
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_a".into()),
                name: Some("semantic_search".into()),
                args_delta: "{\"query\":".into(),
            },
        ),
        (
            Duration::ZERO,
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "\"roadmap\"}".into(),
            },
        ),
        done(),
    ]);
    c.llm.push(vec![
        text("The mobile app ships in Q2 "),
        text("[S1]."),
        done(),
    ]);

    let conv = c.create().await;
    let (s, ack) = c.send(conv, "What is on our roadmap?").await;
    assert_eq!(s, StatusCode::ACCEPTED, "{ack}");
    let reply = ack["assistantMessageId"].as_str().unwrap().to_owned();
    assert!(ack["messageId"].is_string());

    let m = c.wait_reply(conv, &reply).await;
    assert_eq!(m["status"], "completed", "{m}");
    assert_eq!(m["role"], "assistant");
    assert_eq!(m["content"], "The mobile app ships in Q2 [S1].");
    let sources: Value = serde_json::from_str(m["sourcesJson"].as_str().unwrap()).unwrap();
    assert_eq!(sources[0]["marker"], "S1");
    assert_eq!(sources[0]["title"], "Roadmap 2027");
    assert_eq!(sources[0]["ref"], "doc-1");
    assert_eq!(*c.knowledge.queries.lock().unwrap(), vec!["roadmap"]);

    // Second round received the tool result; the request carried the page context and user id.
    let reqs = c.llm.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].system.as_deref().unwrap().contains("documents"));
    assert_eq!(
        reqs[0].metadata.get("user_id").map(String::as_str),
        Some(c.user.as_uuid().to_string().as_str())
    );
    assert!(matches!(reqs[1].tool_choice, ToolChoice::Auto));
    let has_result = reqs[1].messages.iter().any(|msg| {
        msg.parts.iter().any(|p| {
            matches!(p, Part::ToolResult { content, is_error: false, .. } if content.contains("Launch the mobile app"))
        })
    });
    assert!(has_result);

    // Title comes from the first prompt; the list shows the conversation.
    let (s, list) = c
        .call(
            Method::GET,
            &format!("/assistant/conversations?workspaceId={}", c.ws),
            c.user,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == conv.to_string())
        .cloned()
        .unwrap();
    assert_eq!(row["title"], "What is on our roadmap?");
    assert!(row["lastMessageAt"].is_string());
}

#[tokio::test]
async fn streams_events_over_sse() {
    let Some(c) = ctx().await else { return };
    c.llm.push(vec![text("Hello "), text("there."), done()]);
    let conv = c.create().await;

    let resp = c
        .router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/assistant/conversations/{conv}/events"))
                .header("authorization", format!("Bearer {}", c.token(c.user)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("event: ready"));

    let (s, _) = c.send(conv, "hi").await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let mut seen = String::new();
    let read = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(chunk) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if seen.contains("event: AssistantMessageCompleted") {
                break;
            }
        }
    })
    .await;
    assert!(read.is_ok(), "no completion event; got: {seen}");
    assert!(seen.contains("event: AssistantMessageStarted"));
    assert!(seen.contains("event: AssistantMessageChunk"));
    assert!(seen.contains("Hello there."));
    assert!(seen.contains("\nid: "));
}

#[tokio::test]
async fn enforces_membership_and_ownership() {
    let Some(c) = ctx().await else { return };
    let stranger = UserId::from_uuid(Uuid::now_v7());
    let (s, _) = c
        .call(
            Method::GET,
            &format!("/assistant/conversations?workspaceId={}", c.ws),
            stranger,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = c
        .call(
            Method::POST,
            "/assistant/conversations",
            stranger,
            Some(json!({ "workspaceId": c.ws })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let conv = c.create().await;
    let (s, _) = c
        .call(
            Method::GET,
            &format!("/assistant/conversations/{conv}"),
            stranger,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = c
        .call(
            Method::POST,
            &format!("/assistant/conversations/{conv}/messages"),
            stranger,
            Some(json!({ "content": "hi" })),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = c.call(Method::GET, "/assistant/tools", c.user, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = c
        .call(
            Method::GET,
            "/assistant/conversations?workspaceId=x",
            c.user,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_invalid_messages() {
    let Some(c) = ctx().await else { return };
    let conv = c.create().await;
    let uri = format!("/assistant/conversations/{conv}/messages");
    let (s, v) = c
        .call(
            Method::POST,
            &uri,
            c.user,
            Some(json!({ "content": "see file", "attachments": [{ "id": "x" }] })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "ATTACHMENTS_NOT_SUPPORTED");
    let (s, v) = c.send(conv, "   ").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "EMPTY_MESSAGE");
    let (s, v) = c.send(conv, &"x".repeat(8_001)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "MESSAGE_TOO_LONG");
}

#[tokio::test]
async fn one_reply_at_a_time_and_stop_keeps_partial_text() {
    let Some(c) = ctx().await else { return };
    c.llm.push(vec![
        text("Partial"),
        (
            Duration::from_secs(20),
            StreamEvent::TextDelta(" never".into()),
        ),
        done(),
    ]);
    let conv = c.create().await;
    let (s, ack) = c.send(conv, "long answer please").await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let reply = ack["assistantMessageId"].as_str().unwrap().to_owned();

    let (s, v) = c.send(conv, "another").await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "REPLY_IN_PROGRESS");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let (s, _) = c
        .call(
            Method::POST,
            &format!("/assistant/conversations/{conv}/messages/{reply}/stop"),
            c.user,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let m = c.wait_reply(conv, &reply).await;
    assert_eq!(m["status"], "completed", "{m}");
    assert_eq!(m["content"], "Partial");
}

#[tokio::test]
async fn external_model_needs_workspace_consent() {
    let Some(c) = ctx_with(false).await else {
        return;
    };
    let conv = c.create().await;
    let (_, ack) = c.send(conv, "hi").await;
    let reply = ack["assistantMessageId"].as_str().unwrap().to_owned();
    let m = c.wait_reply(conv, &reply).await;
    assert_eq!(m["status"], "failed");
    assert!(c.llm.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn archive_hides_conversation() {
    let Some(c) = ctx().await else { return };
    let conv = c.create().await;
    let (s, _) = c
        .call(
            Method::DELETE,
            &format!("/assistant/conversations/{conv}"),
            c.user,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, list) = c
        .call(
            Method::GET,
            &format!("/assistant/conversations?workspaceId={}", c.ws),
            c.user,
            None,
        )
        .await;
    assert!(list.as_array().unwrap().is_empty());
    let (s, _) = c
        .call(
            Method::GET,
            &format!("/assistant/conversations/{conv}"),
            c.user,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
