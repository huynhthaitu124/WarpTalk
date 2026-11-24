//! Copilot end to end (ADR 0001): the real API router (room, workspace, copilot, transcript
//! read, settings, features) on Postgres + Redis; what the copilot API wrote to `stt:results`
//! is decoded and consumed by the REAL transcript stream handler and the REAL suggestion worker
//! (the consumers' own types, not fakes of them), then read back through `GET /transcripts/{room}`.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use redis::aio::ConnectionManager;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_assistant::suggestion::engine::Admission;
use warptalk_assistant::suggestion::{
    EngineDeps, LlmResolver, NoopRetriever, Purpose, RedisConsentPolicy, RedisRoomSignals,
    ResolveError, ResolvedLlm, RoomDirectory, RoomInfo, RoomSignals, SettingsConfigSource,
    SuggestionEngine, SuggestionState, SuggestionWorker, register_config,
};
use warptalk_bus::{StreamHandler, StreamMessage};
use warptalk_config::JwtConfig;
use warptalk_contracts::events::SttResult;
use warptalk_copilot::infrastructure::openai_stt::{OpenAiSttConfig, OpenAiSttTokenIssuer};
use warptalk_identity::VerificationMailer;
use warptalk_kernel::{AppError, AppResult, RoomId, SystemClock, UserId, WorkspaceId};
use warptalk_settings::{NS_SUGGESTION, Scope, SettingsStore, register_defaults};
use warptalk_transcript::application::ports::{
    AccessTarget, AlwaysSaveRoomPolicy, RoomDetails, RoomHost, TranscriptReadAccess,
    WorkspaceAiPolicy,
};
use warptalk_transcript::infrastructure::consumers::SttResultsHandler;
use warptalk_transcript::{TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct CaptureMailer(Mutex<Vec<(String, String)>>);

#[async_trait]
impl VerificationMailer for CaptureMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push((email.to_owned(), token.to_owned()));
        Ok(())
    }
}

struct Env {
    app: Router,
    pool: sqlx::PgPool,
    redis: ConnectionManager,
    jwt: JwtKeys,
    mailer: Arc<CaptureMailer>,
    openai: MockServer,
}

struct User {
    id: UserId,
    token: String,
}

async fn env() -> Option<Env> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        eprintln!("DATABASE_URL / REDIS_URL not set; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 8).await.unwrap();
    warptalk_db::run_migrations(&pool, &warptalk_migrator_sets())
        .await
        .unwrap();
    let cfg = JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    };
    let jwt = JwtKeys::from_config(&cfg);
    let mailer = Arc::new(CaptureMailer::default());
    let openai = MockServer::start().await;
    let issuer = OpenAiSttTokenIssuer::new(OpenAiSttConfig {
        api_key: "sk-platform-test".into(),
        base_url: openai.uri(),
        model: "gpt-4o-transcribe".into(),
    })
    .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let app = warptalk_api::build_router(
        pool.clone(),
        jwt.clone(),
        &cfg,
        &[],
        mailer.clone(),
        Some(redis.clone()),
        warptalk_api::ApiOptions {
            stt_token_issuer: Some(Arc::new(issuer)),
            ..warptalk_api::ApiOptions::default()
        },
    )
    .await;
    Some(Env {
        app,
        pool,
        redis,
        jwt,
        mailer,
        openai,
    })
}

/// The migrator's own registry, so the test cannot drift from what is deployed.
fn warptalk_migrator_sets() -> Vec<warptalk_db::ModuleMigrations> {
    vec![
        warptalk_db::OUTBOX_MIGRATIONS,
        warptalk_settings::MIGRATIONS,
        warptalk_identity::MIGRATIONS,
        warptalk_workspace::MIGRATIONS,
        warptalk_room::MIGRATIONS,
        warptalk_meeting::MIGRATIONS,
        warptalk_transcript::MIGRATIONS,
        warptalk_knowledge::MIGRATIONS,
        warptalk_assistant::MIGRATIONS,
        warptalk_copilot::MIGRATIONS,
    ]
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/v1{uri}"));
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

impl Env {
    async fn user(&self, name: &str) -> User {
        let email = format!("cp-{}@example.test", Uuid::new_v4().simple());
        let pw = "S3cret-pass!";
        let (s, b) = call(
            &self.app,
            "POST",
            "/auth/register",
            Some(json!({"email": email, "password": pw, "fullName": name})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        let id: Uuid = b["userId"].as_str().unwrap().parse().unwrap();
        let token = self
            .mailer
            .0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(e, _)| *e == email)
            .map(|(_, t)| t.clone())
            .unwrap();
        let (s, _) = call(
            &self.app,
            "POST",
            "/auth/verify-email",
            Some(json!({"token": token})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, b) = call(
            &self.app,
            "POST",
            "/auth/login",
            Some(json!({"email": email, "password": pw})),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        User {
            id: UserId::from(id),
            token: b["accessToken"].as_str().unwrap().to_owned(),
        }
    }

    async fn workspace(&self, owner: &User) -> WorkspaceId {
        let (s, b) = call(
            &self.app,
            "POST",
            "/workspaces",
            Some(json!({"name": format!("CP {}", Uuid::new_v4().simple())})),
            Some(&owner.token),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        WorkspaceId::from(b["id"].as_str().unwrap().parse::<Uuid>().unwrap())
    }

    async fn add_member(&self, ws: WorkspaceId, user: &User) {
        sqlx::query(
            "INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) \
             VALUES ($1, $2, $3, $4, now())",
        )
        .bind(Uuid::now_v7())
        .bind(ws.as_uuid())
        .bind(user.id.as_uuid())
        .bind(warptalk_workspace::domain::role_id(warptalk_kernel::WorkspaceRole::Member))
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn stream(&self, key: &str) -> Vec<StreamMessage> {
        let mut conn = self.redis.clone();
        let reply: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
            .arg(key)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        reply
            .ids
            .into_iter()
            .map(|e| StreamMessage {
                stream: key.to_owned(),
                id: e.id.clone(),
                fields: e
                    .map
                    .iter()
                    .filter_map(|(k, v)| {
                        Some((k.clone(), redis::from_redis_value::<String>(v).ok()?))
                    })
                    .collect::<HashMap<_, _>>(),
            })
            .collect()
    }

    async fn get_key(&self, key: &str) -> Option<String> {
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut self.redis.clone())
            .await
            .unwrap()
    }
}

// ---- consumers built from the real types ------------------------------------------------

struct PgRooms(sqlx::PgPool);

#[async_trait]
impl warptalk_transcript::application::ports::RoomDirectory for PgRooms {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomDetails>> {
        let ws: Option<Uuid> = sqlx::query_scalar(
            "SELECT workspace_id FROM translation_room.translation_rooms WHERE id = $1",
        )
        .bind(room.as_uuid())
        .fetch_optional(&self.0)
        .await
        .unwrap();
        Ok(ws.map(|w| RoomDetails {
            workspace_id: w.into(),
        }))
    }
}

struct NoopRoomHost;

#[async_trait]
impl RoomHost for NoopRoomHost {
    async fn effective_host(&self, _: RoomId) -> AppResult<Option<UserId>> {
        Ok(None)
    }
}

struct NoopReadAccess;

#[async_trait]
impl TranscriptReadAccess for NoopReadAccess {
    async fn can_read(&self, _: UserId, _: AccessTarget) -> AppResult<bool> {
        Ok(false)
    }
}

struct NoRetrieval;

#[async_trait]
impl WorkspaceAiPolicy for NoRetrieval {
    async fn ai_retrieval_allowed(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(false)
    }
    async fn external_llm_allowed(&self, _: WorkspaceId) -> AppResult<bool> {
        Ok(false)
    }
}

struct SuggestRooms(WorkspaceId);

#[async_trait]
impl RoomDirectory for SuggestRooms {
    async fn room_info(&self, _: RoomId) -> Option<RoomInfo> {
        Some(RoomInfo {
            workspace_id: self.0,
            active: true,
            ai_suggest_enabled: true,
        })
    }
}

struct NoLlm;

#[async_trait]
impl LlmResolver for NoLlm {
    async fn for_purpose(&self, _: WorkspaceId, _: Purpose) -> Result<ResolvedLlm, ResolveError> {
        Err(ResolveError::NotConfigured)
    }
}

fn transcript_handler(e: &Env) -> SttResultsHandler {
    let module = TranscriptModule::new(TranscriptDeps {
        pool: e.pool.clone(),
        redis: e.redis.clone(),
        jwt: e.jwt.clone(),
        clock: Arc::new(SystemClock),
        rooms: Arc::new(PgRooms(e.pool.clone())),
        room_policy: Arc::new(AlwaysSaveRoomPolicy),
        workspace_ai: Arc::new(NoRetrieval),
        host: Arc::new(NoopRoomHost),
        read_access: Arc::new(NoopReadAccess),
    });
    SttResultsHandler::new(module.ingest())
}

async fn suggestion_worker(e: &Env, ws: WorkspaceId) -> Arc<SuggestionWorker> {
    let store = SettingsStore::new(e.pool.clone(), Some(e.redis.clone()));
    register_defaults(&store);
    register_config(&store);
    store
        .put(
            Scope::Workspace(ws),
            NS_SUGGESTION,
            json!({"enabled": true}),
            None,
        )
        .await
        .unwrap();
    let engine = SuggestionEngine::new(EngineDeps {
        rooms: Arc::new(SuggestRooms(ws)),
        signals: Arc::new(RedisRoomSignals::new(e.redis.clone())),
        consent: Arc::new(RedisConsentPolicy::new(e.redis.clone())),
        llm: Arc::new(NoLlm),
        knowledge: Arc::new(NoopRetriever),
        config: Arc::new(SettingsConfigSource::new(store)),
        clock: Arc::new(SystemClock),
        state: SuggestionState::new(e.redis.clone()),
        redis: e.redis.clone(),
    });
    Arc::new(SuggestionWorker::new(Arc::new(engine), 2))
}

fn seg(id: &str, channel: &str, text: &str, start: i64, end: i64) -> Value {
    json!({"clientSegmentId": id, "channel": channel, "text": text, "language": "en",
           "startMs": start, "endMs": end, "confidence": 0.92, "isFinal": true})
}

#[tokio::test]
async fn features_endpoint() {
    let Some(e) = env().await else { return };
    // Anonymous: platform defaults.
    let (s, b) = call(&e.app, "GET", "/features", None, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        b,
        json!({"translationMeetings": false, "dubbing": false, "voiceClone": false,
               "externalBridge": false, "copilot": true})
    );
    let owner = e.user("Owner").await;
    let outsider = e.user("Outsider").await;
    let ws = e.workspace(&owner).await;
    let (s, b) = call(
        &e.app,
        "GET",
        &format!("/features?workspaceId={ws}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["translationMeetings"], false);
    let (s, _) = call(
        &e.app,
        "GET",
        &format!("/features?workspaceId={ws}"),
        None,
        Some(&outsider.token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&e.app, "GET", "/features", None, Some("not-a-token")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // A workspace admin turns translation on for the workspace only.
    let (s, b) = call(
        &e.app,
        "PUT",
        &format!("/settings/features?scope=workspace&id={ws}"),
        Some(json!({"translation_meetings": true})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/features?workspaceId={ws}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(
        (b["translationMeetings"].as_bool(), b["copilot"].as_bool()),
        (Some(true), Some(true))
    );
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/features?workspaceId={ws}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        b["translationMeetings"], false,
        "anonymous callers always get the platform flags"
    );
}

#[tokio::test]
async fn copilot_session_end_to_end() {
    let Some(e) = env().await else { return };
    let host = e.user("Host").await;
    let colleague = e.user("Colleague").await;
    let ws = e.workspace(&host).await;
    e.add_member(ws, &colleague).await;

    // Create: IN_PROGRESS immediately, host = caller.
    let (s, b) = call(
        &e.app,
        "POST",
        "/copilot/sessions",
        Some(json!({"workspaceId": ws.to_string(), "title": "Pricing call", "platform": "zoom",
                    "meetingUrl": "https://zoom.us/j/123", "languages": {"me": "vi", "others": "en"}})),
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["status"], "IN_PROGRESS");
    assert_eq!(b["hostId"], host.id.to_string());
    let id = b["id"].as_str().unwrap().to_owned();
    let room: RoomId = id.parse::<Uuid>().unwrap().into();

    // It is a room of type COPILOT_SESSION; nobody can join it.
    let (s, b) = call(
        &e.app,
        "GET",
        &format!("/translation-rooms/{id}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["roomType"].as_str(), b["status"].as_str()),
        (Some("COPILOT_SESSION"), Some("IN_PROGRESS"))
    );
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/translation-rooms/{id}/join"),
        Some(json!({"speakLanguage": "en", "listenLanguage": "en"})),
        Some(&colleague.token),
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::CONFLICT, Some("NOT_JOINABLE")),
        "{b}"
    );

    // The assistant's keys exist: ai_policy (fail-closed external LLM by default) and the host's
    // reader language.
    let policy: Value = serde_json::from_str(
        &e.get_key(&format!("translationRoom:{id}:ai_policy"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(policy["allow_external_llm"], false);
    let signals = RedisRoomSignals::new(e.redis.clone());
    assert_eq!(signals.reader_languages(room).await, vec!["vi".to_owned()]);

    // The host's own sessions list, newest first; a colleague sees none of them.
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/copilot/sessions?workspaceId={ws}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(b["items"][0]["id"], id.as_str());
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/copilot/sessions?workspaceId={ws}"),
        None,
        Some(&colleague.token),
    )
    .await;
    assert_eq!(b["total"], 0);

    // Ingest.
    let question = "Can you walk me through the pricing for the enterprise plan please?";
    let seg_uri = format!("/copilot/sessions/{id}/segments");
    let (s, b) = call(
        &e.app,
        "POST",
        &seg_uri,
        Some(json!({"segments": [
            seg("c-1", "others", question, 1_000, 4_200),
            seg("c-2", "me", "Sure, the enterprise plan starts at ten seats.", 5_000, 8_100)
        ]})),
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["accepted"], 2);
    let (_, b2) = call(
        &e.app,
        "POST",
        &seg_uri,
        Some(json!({"segments": [seg("c-1", "others", question, 1_000, 4_200)]})),
        Some(&host.token),
    )
    .await;
    assert_eq!(
        (b2["accepted"].as_u64(), b2["duplicates"].as_u64()),
        (Some(0), Some(1))
    );
    let (s, _) = call(
        &e.app,
        "POST",
        &seg_uri,
        Some(json!({"segments": [seg("c-3", "me", "x", 1, 2)]})),
        Some(&colleague.token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "only the host ingests");

    // Both streams carry exactly the two segments, in the pipeline's contract.
    let copy = e.stream(&format!("stt:results:{id}")).await;
    assert_eq!(
        copy.len(),
        2,
        "per-meeting copy, duplicate not re-published"
    );
    let global = e.stream("stt:results").await;
    let ours: Vec<_> = global
        .iter()
        .filter(|m| {
            m.json::<SttResult>()
                .map(|r| r.meeting_id == room)
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(ours.len(), 2);

    // The REAL transcript handler persists them...
    let handler = transcript_handler(&e);
    for m in &copy {
        handler.handle(m).await.unwrap();
    }
    // ...and the REAL suggestion worker admits the others' question (and takes the rest).
    let worker = suggestion_worker(&e, ws).await;
    let first: SttResult = copy[0].json().unwrap();
    match worker.engine().admit(&first).await {
        Admission::Admitted(_) => {}
        Admission::Rejected(why) => {
            panic!("suggestion engine rejected the copilot segment: {why:?}")
        }
    }
    for m in &copy {
        worker.handle(m).await.unwrap();
    }
    worker.drain().await;

    // Read back through the HTTP API.
    let (s, t) = call(
        &e.app,
        "GET",
        &format!("/transcripts/{id}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{t}");
    let segs = t["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2, "{t}");
    let (others, me) = (&segs[0], &segs[1]);
    assert_eq!(others["speakerName"], "Others");
    assert_eq!(me["speakerName"], "Tôi");
    assert_eq!(others["originalText"], question);
    assert_eq!(
        (others["startTimeMs"].as_i64(), others["endTimeMs"].as_i64()),
        (Some(1_000), Some(4_200))
    );
    assert_eq!(
        (me["startTimeMs"].as_i64(), me["endTimeMs"].as_i64()),
        (Some(5_000), Some(8_100))
    );
    assert_eq!(others["isFinal"], true);
    let host_participant: Uuid = sqlx::query_scalar(
        "SELECT id FROM translation_room.translation_room_participants \
         WHERE translation_room_id = $1 AND role = 'host'",
    )
    .bind(room.as_uuid())
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert_eq!(me["speakerParticipantId"], host_participant.to_string());
    assert_ne!(others["speakerParticipantId"], me["speakerParticipantId"]);
    assert_eq!(t["totalSegments"], 2);
    let (s, _) = call(
        &e.app,
        "GET",
        &format!("/transcripts/{id}"),
        None,
        Some(&colleague.token),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "transcripts of a session are for its host and workspace admins"
    );

    // Post-session summary endpoint answers for a copilot room (nothing generated yet).
    let (s, b) = call(
        &e.app,
        "GET",
        &format!("/meetings/{id}/summary"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(b["status"].is_string(), "{b}");

    // Ephemeral STT token (OpenAI faked with wiremock; the request carries the platform key).
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .and(header("authorization", "Bearer sk-platform-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"value": "ek_e2e", "expires_at": 1_900_000_000})),
        )
        .mount(&e.openai)
        .await;
    let tok_uri = format!("/copilot/sessions/{id}/stt-token");
    let (s, b) = call(&e.app, "POST", &tok_uri, None, Some(&host.token)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["provider"].as_str(), b["token"].as_str()),
        (Some("openai"), Some("ek_e2e"))
    );
    assert!(b["websocketUrl"].as_str().unwrap().contains("/v1/realtime"));
    let (s, _) = call(&e.app, "POST", &tok_uri, None, Some(&colleague.token)).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "a workspace member who is not the host cannot mint"
    );
    // Redis rate limit: 10 per minute per user (one already used).
    for _ in 0..9 {
        let (s, _) = call(&e.app, "POST", &tok_uri, None, Some(&host.token)).await;
        assert_eq!(s, StatusCode::OK);
    }
    let (s, b) = call(&e.app, "POST", &tok_uri, None, Some(&host.token)).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{b}");

    // End: session and room ENDED, the marker reaches the consumers, ai keys are cleared.
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/copilot/sessions/{id}/end"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "ENDED");
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/translation-rooms/{id}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(b["status"], "ENDED");
    let after = e.stream(&format!("stt:results:{id}")).await;
    let markers: Vec<SttResult> = after
        .iter()
        .filter_map(|m| m.json::<SttResult>().ok())
        .filter(|r| r.is_control_marker())
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "one __MEETING_END__ marker on the per-meeting copy"
    );
    for m in &after {
        handler.handle(m).await.unwrap(); // the marker is ignored by the transcript, never an error
        worker.handle(m).await.unwrap();
    }
    assert!(
        e.get_key(&format!("translationRoom:{id}:ai_policy"))
            .await
            .is_none()
    );
    let (s, b) = call(
        &e.app,
        "POST",
        &seg_uri,
        Some(json!({"segments": [seg("c-9", "me", "late", 1, 2)]})),
        Some(&host.token),
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::CONFLICT, Some("SESSION_NOT_IN_PROGRESS"))
    );
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/copilot/sessions/{id}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(b["status"], "ENDED");
    // Let any straggling redis pipeline settle before the process exits.
    tokio::time::sleep(Duration::from_millis(50)).await;
}
