//! Cross-component lifecycle tests: the real room/meeting/workspace modules, the real Redis
//! publishers and a recording LiveKit `RoomService`, with the consumers' own types decoding what
//! the producers wrote (B-08, B-09, B-10, B-12, B-13 and suggestion delivery).
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_assistant::suggestion::{FollowUp, FollowUpEvent, SuggestionResult};
use warptalk_config::JwtConfig;
use warptalk_contracts::channels::TRANSLATION_ROOM_COMMANDS;
use warptalk_contracts::events::{RoomCommand, SttResult};
use warptalk_identity::VerificationMailer;
use warptalk_kernel::{AppError, RoomId, UserId, WorkspaceId, WorkspaceRole};
use warptalk_livekit::{InMemoryRoomService, LiveKitConfig, RoomServiceCall};
use warptalk_web::JwtKeys;

#[derive(Default)]
struct CaptureMailer(Mutex<Vec<(String, String)>>);

#[async_trait::async_trait]
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
    redis_url: String,
    lk: Arc<InMemoryRoomService>,
    mailer: Arc<CaptureMailer>,
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
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_db::OUTBOX_MIGRATIONS,
            warptalk_settings::MIGRATIONS,
            warptalk_identity::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
            warptalk_room::MIGRATIONS,
            warptalk_meeting::MIGRATIONS,
            warptalk_transcript::MIGRATIONS,
            warptalk_knowledge::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let cfg = JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    };
    let mailer = Arc::new(CaptureMailer::default());
    let lk = Arc::new(InMemoryRoomService::default());
    let app = warptalk_api::build_router(
        pool.clone(),
        JwtKeys::from_config(&cfg),
        &cfg,
        &[],
        mailer.clone(),
        Some(warptalk_bus::connect(&redis_url).await.unwrap()),
        warptalk_api::ApiOptions {
            livekit: Some(LiveKitConfig {
                url: "wss://lk.test".to_owned(),
                api_key: "APIkey".to_owned(),
                api_secret: "lk-secret-lk-secret-lk-secret-0001".to_owned(),
            }),
            room_service: Some(lk.clone()),
            vault: Some(Arc::new(
                warptalk_ai_plane::KeyVault::from_base64(
                    "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
                    1,
                )
                .unwrap(),
            )),
            ..warptalk_api::ApiOptions::default()
        },
    )
    .await;
    Some(Env {
        app,
        pool,
        redis_url,
        lk,
        mailer,
    })
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> axum::response::Response {
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
    app.clone().oneshot(req).await.unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let resp = raw(app, method, uri, body, bearer).await;
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
        let email = format!("lc-{}@example.test", Uuid::new_v4().simple());
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
            Some(json!({"name": format!("LC {}", Uuid::new_v4().simple())})),
            Some(&owner.token),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        WorkspaceId::from(b["id"].as_str().unwrap().parse::<Uuid>().unwrap())
    }

    async fn add_member(&self, ws: WorkspaceId, user: &User, role: WorkspaceRole) {
        sqlx::query(
            "INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) \
             VALUES ($1, $2, $3, $4, now())",
        )
        .bind(Uuid::now_v7())
        .bind(ws.as_uuid())
        .bind(user.id.as_uuid())
        .bind(warptalk_workspace::domain::role_id(role))
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn redis(&self) -> redis::aio::ConnectionManager {
        warptalk_bus::connect(&self.redis_url).await.unwrap()
    }

    async fn get_key(&self, key: &str) -> Option<String> {
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut self.redis().await)
            .await
            .unwrap()
    }
}

fn suggestion(room: RoomId, ws: WorkspaceId, id: &str, language: &str) -> SuggestionResult {
    SuggestionResult {
        suggestion_id: id.to_owned(),
        meeting_id: room,
        workspace_id: ws,
        segment_id: "seg-1".to_owned(),
        kind: "suggestion".to_owned(),
        category: "risk".to_owned(),
        severity: "info".to_owned(),
        content: format!("content {id}"),
        detail: String::new(),
        confidence: 0.9,
        language: language.to_owned(),
        token_count: 3,
        sources: vec![],
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        ttl_seconds: 120,
        conflict: false,
        follow_up: None,
        actions: vec![],
        prompt_version: "t".to_owned(),
    }
}

/// Reads SSE frames until `n` `suggestion` events were seen (or the timeout hits).
async fn read_events(resp: axum::response::Response, n: usize) -> Vec<(String, Value)> {
    let mut body = resp.into_body().into_data_stream();
    let mut buf = String::new();
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while out.len() < n {
        let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, body.next()).await else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        while let Some(pos) = buf.find("\n\n") {
            let frame: String = buf.drain(..pos + 2).collect();
            let mut id = None;
            let mut data = None;
            let mut event = None;
            for line in frame.lines() {
                if let Some(v) = line.strip_prefix("id:") {
                    id = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_owned());
                }
            }
            if event.as_deref() == Some("suggestion") {
                out.push((id.unwrap(), serde_json::from_str(&data.unwrap()).unwrap()));
            }
        }
    }
    out
}

/// B-08 + B-10 + B-09 + B-12 + B-13 through the real API.
#[tokio::test]
async fn end_kick_listener_and_ai_keys() {
    let Some(e) = env().await else { return };
    let host = e.user("LC Host").await;
    let guest = e.user("LC Guest").await;
    let ws = e.workspace(&host).await;
    e.add_member(ws, &guest, WorkspaceRole::Member).await;

    // B-13: the settings writer persists the three flags where the readers look.
    let settings = format!("/workspaces/{ws}/settings");
    let (s, b) = call(
        &e.app,
        "PUT",
        &settings,
        Some(json!({"aiRetrievalAllowed": true, "externalLlmAllowed": true, "aiSuggestEnabled": true})),
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let worker_view: (bool, bool) = sqlx::query_as(
        "SELECT COALESCE(settings -> 'aiRetrievalAllowed' = 'true'::jsonb, false), \
                COALESCE(settings -> 'externalLlmAllowed' = 'true'::jsonb, false) \
         FROM workspace.workspaces WHERE id = $1",
    )
    .bind(ws.as_uuid())
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert_eq!(worker_view, (true, true));

    let (s, b) = call(
        &e.app,
        "POST",
        "/translation-rooms",
        Some(json!({"workspaceId": ws, "title": "LC", "requiresApproval": false, "sourceLanguage": "vi", "targetLanguages": ["en"]})),
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let room: RoomId = RoomId::from(b["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    let rid = room.to_string();

    // Real consumer of the room command channel, subscribed before anything is published.
    let client = redis::Client::open(e.redis_url.as_str()).unwrap();
    let mut pubsub = client.get_async_pubsub().await.unwrap();
    pubsub.subscribe(TRANSLATION_ROOM_COMMANDS).await.unwrap();
    let mut commands = pubsub.into_on_message();

    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/translation-rooms/{room}/start"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    // B-12: ai_policy is written on room start from the workspace flags.
    let policy: Value = serde_json::from_str(
        &e.get_key(&format!("translationRoom:{rid}:ai_policy"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(policy["allow_external_llm"], true);

    // A settings change refreshes the live room's ai_policy.
    let (s, _) = call(
        &e.app,
        "PUT",
        &settings,
        Some(json!({"externalLlmAllowed": false})),
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let policy: Value = serde_json::from_str(
        &e.get_key(&format!("translationRoom:{rid}:ai_policy"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(policy["allow_external_llm"], false);
    assert_eq!(policy["ai_retrieval_allowed"], true);

    // Guest joins the room: reader languages are published with the routes.
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/translation-rooms/{room}/join"),
        Some(json!({"speakLanguage": "en", "listenLanguage": "en"})),
        Some(&guest.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let langs: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(format!("translationRoom:{rid}:languages"))
        .query_async(&mut e.redis().await)
        .await
        .unwrap();
    assert!(!langs.is_empty(), "languages hash written");
    assert!(langs.values().any(|l| l == "en"), "{langs:?}");

    // B-09: main session and listener seat coexist with different identities.
    let join = format!("/meetings/{room}/join");
    let (s, main) = call(&e.app, "POST", &join, Some(json!({})), Some(&guest.token)).await;
    assert_eq!(s, StatusCode::OK, "{main}");
    assert_eq!(main["participantIdentity"], guest.id.to_string());
    let (s, lst) = call(
        &e.app,
        "POST",
        &join,
        Some(json!({"clientTag": "listener"})),
        Some(&guest.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{lst}");
    assert_eq!(lst["participantIdentity"], format!("{}:listener", guest.id));
    assert_ne!(lst["token"], main["token"]);
    let (s, _) = call(&e.app, "POST", &join, Some(json!({})), Some(&host.token)).await;
    assert_eq!(s, StatusCode::OK);

    // B-10: kick removes every LiveKit connection of the user and new join tokens are refused.
    let (_, parts) = call(
        &e.app,
        "GET",
        &format!("/translation-rooms/{room}/participants"),
        None,
        Some(&host.token),
    )
    .await;
    let pid = parts
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["userId"] == guest.id.to_string().as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, b) = call(
        &e.app,
        "PUT",
        &format!("/translation-rooms/{room}/participants/{pid}/kick"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let calls = e.lk.calls();
    for identity in [
        guest.id.to_string(),
        format!("{}:listener", guest.id),
        format!("{}:bridge-listener", guest.id),
    ] {
        assert!(
            calls.contains(&RoomServiceCall::RemoveParticipant {
                room: rid.clone(),
                identity: identity.clone(),
            }),
            "missing RemoveParticipant {identity}: {calls:?}"
        );
    }
    for tag in [json!({}), json!({"clientTag": "listener"})] {
        let (s, b) = call(&e.app, "POST", &join, Some(tag), Some(&guest.token)).await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "kicked user gets no new token: {b}"
        );
    }
    assert!(
        !calls.contains(&RoomServiceCall::RemoveParticipant {
            room: rid.clone(),
            identity: host.id.to_string(),
        }),
        "the host was not removed"
    );

    // B-08: ending the meeting ends the translation room even though LiveKit is configured.
    let (s, b) = call(
        &e.app,
        "POST",
        &format!("/meetings/{room}/end"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    assert!(
        e.lk.calls()
            .contains(&RoomServiceCall::DeleteRoom(rid.clone())),
        "LiveKit DeleteRoom kept"
    );
    let (s, b) = call(
        &e.app,
        "GET",
        &format!("/translation-rooms/{room}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "ENDED");

    // `RoomEnded` reached the command channel (what the pipeline's host consumes).
    let mut seen_end = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, commands.next()).await {
        let payload: String = msg.get_payload().unwrap();
        if let Ok(RoomCommand::RoomEnded { room_id }) =
            serde_json::from_str::<RoomCommand>(&payload)
            && room_id == room
        {
            seen_end = true;
            break;
        }
    }
    assert!(seen_end, "RoomEnded published");

    // The `__MEETING_END__` marker is on stt:results (global and per-meeting), decodable by the
    // transcript/suggestion consumers' `SttResult`.
    let mut conn = e.redis().await;
    for stream in [format!("stt:results:{rid}"), "stt:results".to_owned()] {
        let entries: redis::streams::StreamRangeReply = redis::cmd("XREVRANGE")
            .arg(&stream)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(50)
            .query_async(&mut conn)
            .await
            .unwrap();
        let markers: Vec<SttResult> = entries
            .ids
            .iter()
            .filter_map(|id| {
                let raw: String = redis::from_redis_value(id.map.get("data")?).ok()?;
                serde_json::from_str::<SttResult>(&raw).ok()
            })
            .filter(|r| r.meeting_id == room)
            .collect();
        assert_eq!(markers.len(), 1, "one marker on {stream}");
        let m = &markers[0];
        assert_eq!(m.text, "__MEETING_END__");
        assert_eq!(m.speaker_id, "system");
        assert_eq!(m.language, "system");
        assert!(m.is_control_marker() && m.is_final_chunk);
    }
    assert!(
        e.get_key(&format!("translationRoom:{rid}:ai_policy"))
            .await
            .is_none(),
        "ai_policy cleared on end"
    );

    // Repeat End is idempotent and writes no second marker.
    let (s, _) = call(
        &e.app,
        "POST",
        &format!("/meetings/{room}/end"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let n: usize = redis::cmd("XLEN")
        .arg(format!("stt:results:{rid}"))
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Ending through the meeting endpoint works for a room nobody joined on the media side, too.
#[tokio::test]
async fn end_without_media_session_publishes_end() {
    let Some(e) = env().await else { return };
    let host = e.user("LC Host2").await;
    let ws = e.workspace(&host).await;
    let (_, b) = call(
        &e.app,
        "POST",
        "/translation-rooms",
        Some(json!({"workspaceId": ws, "title": "LC2", "sourceLanguage": "vi", "targetLanguages": ["en"]})),
        Some(&host.token),
    )
    .await;
    let room = b["id"].as_str().unwrap().to_owned();
    let (s, _) = call(
        &e.app,
        "POST",
        &format!("/translation-rooms/{room}/start"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(
        &e.app,
        "POST",
        &format!("/meetings/{room}/end"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, b) = call(
        &e.app,
        "GET",
        &format!("/translation-rooms/{room}"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(b["status"], "ENDED");
    let n: usize = redis::cmd("XLEN")
        .arg(format!("stt:results:{room}"))
        .query_async(&mut e.redis().await)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Suggestions: producer = the engine's `SuggestionResult::to_fields()` on the per-meeting
/// stream, consumer = the SSE endpoint; follow-ups from `assistant:followups`.
#[tokio::test]
async fn suggestions_stream_and_followups() {
    let Some(e) = env().await else { return };
    let host = e.user("LC Host3").await;
    let outsider = e.user("LC Outsider").await;
    let ws = e.workspace(&host).await;
    let (_, b) = call(
        &e.app,
        "POST",
        "/translation-rooms",
        Some(json!({"workspaceId": ws, "title": "LC3", "sourceLanguage": "vi", "targetLanguages": ["en"]})),
        Some(&host.token),
    )
    .await;
    let room = RoomId::from(b["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    let stream = format!("ai_assistant:results:{room}");
    let mut conn = e.redis().await;
    let publish = |r: SuggestionResult| {
        let mut conn = conn.clone();
        let stream = stream.clone();
        async move {
            let fields = r.to_fields();
            warptalk_bus::xadd(&mut conn, &stream, &fields, 1000)
                .await
                .unwrap()
        }
    };
    let first = publish(suggestion(room, ws, "s1", "en")).await;
    // Expired entries are not replayed.
    let mut old = suggestion(room, ws, "old", "en");
    old.timestamp_ms -= 10 * 60 * 1000;
    publish(old).await;

    // Not a participant: refused. Unknown room: 404.
    let resp = raw(
        &e.app,
        "GET",
        &format!("/meetings/{room}/suggestions/stream"),
        None,
        Some(&outsider.token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = raw(
        &e.app,
        "GET",
        &format!("/meetings/{}/suggestions/stream", RoomId::new()),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = raw(
        &e.app,
        "GET",
        &format!("/meetings/{room}/suggestions/stream"),
        None,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = raw(
        &e.app,
        "GET",
        &format!("/meetings/{room}/suggestions/stream"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    // Publish a live one shortly after connecting: replay (s1) then tail (s2).
    let live = tokio::spawn({
        let mut conn = conn.clone();
        let stream = stream.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            let fields = suggestion(room, ws, "s2", "en").to_fields();
            warptalk_bus::xadd(&mut conn, &stream, &fields, 1000)
                .await
                .unwrap();
        }
    });
    let got = read_events(resp, 2).await;
    live.await.unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].0, first);
    assert_eq!(got[0].1["suggestion_id"], "s1");
    assert_eq!(got[0].1["meeting_id"], room.to_string());
    assert_eq!(got[1].1["suggestion_id"], "s2");

    // Reconnect with Last-Event-ID: only newer entries.
    let req = Request::builder()
        .uri(format!("/api/v1/meetings/{room}/suggestions/stream"))
        .header("authorization", format!("Bearer {}", host.token))
        .header("last-event-id", &first)
        .body(Body::empty())
        .unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    let got = read_events(resp, 1).await;
    assert_eq!(
        got[0].1["suggestion_id"], "s2",
        "continues after the id, skips expired"
    );

    // Follow-ups of this room only.
    let ev = |meeting: RoomId, task: &str| FollowUpEvent {
        suggestion_id: Uuid::now_v7().to_string(),
        meeting_id: meeting,
        workspace_id: ws,
        segment_id: "seg-1".into(),
        category: "action_item".into(),
        follow_up: FollowUp {
            owner: Some("An".into()),
            task: task.into(),
            due: Some("Friday".into()),
            source_segment_ids: vec!["seg-1".into()],
        },
        confidence: 0.8,
        language: "en".into(),
        timestamp_ms: 1,
    };
    for (meeting, task) in [
        (room, "send deck"),
        (RoomId::new(), "other room"),
        (room, "book room"),
    ] {
        warptalk_bus::xadd_json(&mut conn, "assistant:followups", &ev(meeting, task), 10_000)
            .await
            .unwrap();
    }
    let (s, b) = call(
        &e.app,
        "GET",
        &format!("/meetings/{room}/followups"),
        None,
        Some(&host.token),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let tasks: Vec<&str> = b
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["task"].as_str().unwrap())
        .collect();
    assert_eq!(tasks, ["send deck", "book room"]);
    assert_eq!(b[0]["owner"], "An");
    assert_eq!(b[0]["due"], "Friday");
    let (s, _) = call(
        &e.app,
        "GET",
        &format!("/meetings/{room}/followups"),
        None,
        Some(&outsider.token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
