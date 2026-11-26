//! Delivery of AI suggestions and follow-ups to room participants.
//!
//! - `GET /meetings/{room_id}/suggestions/stream` (SSE): forwards `ai_assistant:results:{room}`
//!   entries. On connect the newest `replay` (default 20, max 100) unexpired suggestions are sent
//!   first, then new entries as they arrive. Event: `suggestion`, id = Redis stream id (clients
//!   reconnect with `Last-Event-ID`, nothing is replayed twice), data = the suggestion JSON
//!   (`suggestion_id`, `segment_id`, `category`, `severity`, `content`, `detail`, `language`,
//!   `sources`, `follow_up`, `actions`, ... snake_case, see `SuggestionResult`). `?lang=xx` keeps
//!   only that language's copy.
//! - `GET /meetings/{room_id}/followups`: follow-ups of the room from `assistant:followups`
//!   (05a has no action-item table, so they are read from the stream, newest 5000 entries).
//! - Dev test mode only (`SUGGESTION_TEST_SCRIPT`, refused in production): `GET /dev/test-mode`
//!   tells clients whether to show the script box, and `POST /meetings/{room_id}/dev/transcript`
//!   `{text, language?, speaker?}` publishes one final segment on `stt:results` (and the per-room
//!   copy) so the real suggestion pipeline runs on typed lines. Without test mode both are 404.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures::Stream;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use warptalk_assistant::suggestion::{AI_ASSISTANT_RESULTS, ASSISTANT_FOLLOWUPS, FollowUpEvent};
use warptalk_contracts::events::SttResult;
use warptalk_contracts::streams::{self, STT_RESULTS};
use warptalk_kernel::{AppError, AppResult, RoomId, UserId, WorkspaceRole};
use warptalk_room::RoomQueries;
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};
use warptalk_workspace::WorkspaceQueries;

const DEFAULT_REPLAY: usize = 20;
const MAX_REPLAY: usize = 100;
const POLL: Duration = Duration::from_millis(400);
/// Re-check room access every this many polls (about 10 s).
const ACCESS_RECHECK_POLLS: u32 = 25;
const FOLLOWUP_SCAN: usize = 5000;
const STT_STREAM_MAXLEN: usize = 100_000;
const MAX_TEST_LINE_CHARS: usize = 1000;

#[derive(Clone)]
pub struct SuggestionsState {
    pub redis: ConnectionManager,
    pub jwt: JwtKeys,
    pub rooms: RoomQueries,
    pub workspaces: WorkspaceQueries,
    /// Dev suggestion test mode: mounts the `/dev/...` routes.
    pub test_mode: bool,
}

impl FromRef<SuggestionsState> for JwtKeys {
    fn from_ref(s: &SuggestionsState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: SuggestionsState) -> Router {
    Router::new()
        .route(
            "/meetings/{room_id}/suggestions/stream",
            get(suggestions_stream),
        )
        .route("/meetings/{room_id}/followups", get(followups))
        .route("/dev/test-mode", get(test_mode))
        .route(
            "/meetings/{room_id}/dev/transcript",
            post(inject_transcript),
        )
        .with_state(state)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestModeDto {
    suggestion_script: bool,
}

/// Whether the dev suggestion test mode is on (clients show the script box only then).
async fn test_mode(State(s): State<SuggestionsState>, _user: AuthUser) -> Json<TestModeDto> {
    Json(TestModeDto {
        suggestion_script: s.test_mode,
    })
}

#[derive(Debug, Deserialize)]
struct InjectBody {
    text: String,
    language: Option<String>,
    /// Display name for the line (simulates several speakers); default: the caller.
    speaker: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InjectedDto {
    segment_id: String,
    speaker_id: String,
    speaker_name: Option<String>,
    text: String,
    language: String,
    timestamp_ms: i64,
}

/// Dev test mode: publish a typed line as a final STT segment of the room.
async fn inject_transcript(
    State(s): State<SuggestionsState>,
    user: AuthUser,
    Path(room_id): Path<RoomId>,
    Json(body): Json<InjectBody>,
) -> AppResult<(StatusCode, Json<InjectedDto>)> {
    if !s.test_mode {
        return Err(AppError::not_found("not found"));
    }
    ensure_can_view(&s, room_id, user.user_id()).await?;
    let text: String = body
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEST_LINE_CHARS)
        .collect();
    if text.is_empty() {
        return Err(AppError::validation("TEXT_REQUIRED", "text is required"));
    }
    let language = body
        .language
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty() && l.len() <= 8)
        .unwrap_or_else(|| "en".to_owned());
    let speaker_name = body
        .speaker
        .map(|n| n.trim().chars().take(60).collect::<String>())
        .filter(|n| !n.is_empty());
    let speaker_id = match &speaker_name {
        Some(n) => format!("test:{n}"),
        None => user.user_id().to_string(),
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let seg = SttResult {
        segment_id: uuid::Uuid::now_v7().to_string(),
        meeting_id: room_id,
        speaker_id: speaker_id.clone(),
        text: text.clone(),
        language: language.clone(),
        start_ms: now_ms,
        end_ms: now_ms,
        anchor_ms: None,
        confidence: None,
        chunk_index: Some(0),
        is_final_chunk: true,
        timestamp_ms: Some(now_ms),
        clean_flags: None,
        far_speaker_name: speaker_name.clone(),
        far_speaker_source: speaker_name.as_ref().map(|_| "participant".to_owned()),
        far_speaker_confidence: speaker_name.as_ref().map(|_| 1.0),
    };
    let mut conn = s.redis.clone();
    let copy = streams::per_meeting(STT_RESULTS, room_id);
    for stream in [STT_RESULTS, copy.as_str()] {
        warptalk_bus::xadd_json(&mut conn, stream, &seg, STT_STREAM_MAXLEN)
            .await
            .map_err(redis_err)?;
    }
    tracing::info!(room = %room_id, "test mode: transcript line injected");
    Ok((
        StatusCode::CREATED,
        Json(InjectedDto {
            segment_id: seg.segment_id,
            speaker_id,
            speaker_name,
            text,
            language,
            timestamp_ms: now_ms,
        }),
    ))
}

/// Host, workspace owner/admin, or a participant who is (or was) in the room. Waiting, kicked
/// and rejected users and outsiders are refused (`404` for unknown rooms).
async fn ensure_can_view(s: &SuggestionsState, room: RoomId, user: UserId) -> AppResult<()> {
    let summary = s
        .rooms
        .room_summary(room)
        .await?
        .ok_or_else(|| AppError::not_found("translation room not found"))?;
    if summary.effective_host_id == user || summary.host_id == user {
        return Ok(());
    }
    if matches!(
        s.workspaces.member_role(summary.workspace_id, user).await?,
        Some(WorkspaceRole::Owner | WorkspaceRole::Admin)
    ) {
        return Ok(());
    }
    match s.rooms.participant_summary(room, user).await? {
        Some(p) if matches!(p.status.as_str(), "CONNECTED" | "DISCONNECTED" | "LEFT") => Ok(()),
        _ => Err(AppError::forbidden("not a participant of this meeting")),
    }
}

fn redis_err(e: impl std::fmt::Display) -> AppError {
    AppError::unavailable(format!("event bus unavailable: {e}"))
}

/// `(stream id, data JSON)` of an entry; entries without a parsable `data` field are skipped.
fn entry_data(id: &redis::streams::StreamId) -> Option<(String, Value)> {
    let raw = id
        .map
        .get("data")
        .and_then(|v| redis::from_redis_value::<String>(v).ok())?;
    Some((id.id.clone(), serde_json::from_str(&raw).ok()?))
}

fn stream_id_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| {
        let (ms, seq) = s.split_once('-').unwrap_or((s, "0"));
        (
            ms.parse::<u64>().unwrap_or(0),
            seq.parse::<u64>().unwrap_or(0),
        )
    };
    parse(a) > parse(b)
}

fn expired(v: &Value, now_ms: i64) -> bool {
    let ts = v.get("timestamp_ms").and_then(Value::as_i64).unwrap_or(0);
    let ttl = v.get("ttl_seconds").and_then(Value::as_i64).unwrap_or(0);
    ttl > 0 && ts > 0 && ts + ttl * 1000 < now_ms
}

fn wanted(v: &Value, lang: Option<&str>) -> bool {
    lang.is_none_or(|l| {
        v.get("language")
            .and_then(Value::as_str)
            .is_some_and(|x| x.eq_ignore_ascii_case(l))
    })
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    replay: Option<usize>,
    lang: Option<String>,
}

struct Tail {
    state: SuggestionsState,
    room: RoomId,
    user: UserId,
    key: String,
    cursor: String,
    lang: Option<String>,
    buffered: VecDeque<Event>,
    polls: u32,
}

impl Tail {
    /// Next batch of entries after the cursor (non-blocking, so the shared connection is not held).
    async fn poll(&mut self) -> Result<(), AppError> {
        let mut conn = self.state.redis.clone();
        let reply: redis::streams::StreamReadReply = redis::cmd("XREAD")
            .arg("COUNT")
            .arg(100)
            .arg("STREAMS")
            .arg(&self.key)
            .arg(&self.cursor)
            .query_async(&mut conn)
            .await
            .map_err(redis_err)?;
        for key in reply.keys {
            for id in key.ids {
                self.cursor = id.id.clone();
                if let Some((sid, data)) = entry_data(&id)
                    && wanted(&data, self.lang.as_deref())
                    && !expired(&data, chrono::Utc::now().timestamp_millis())
                {
                    self.buffered.push_back(event(&sid, &data));
                }
            }
        }
        Ok(())
    }
}

fn event(id: &str, data: &Value) -> Event {
    Event::default()
        .event("suggestion")
        .id(id)
        .data(data.to_string())
}

async fn suggestions_stream(
    State(s): State<SuggestionsState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let user = auth.user_id();
    ensure_can_view(&s, room, user).await?;
    let key = format!("{AI_ASSISTANT_RESULTS}:{room}");
    let lang = q.lang.filter(|l| !l.trim().is_empty());
    let now_ms = chrono::Utc::now().timestamp_millis();
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
        .map(str::to_owned);

    let mut buffered = VecDeque::new();
    let cursor = match last_event_id {
        // Reconnect: continue exactly after the last event the client saw.
        Some(id) => id,
        None => {
            // Cursor taken before the replay read, so nothing published in between is lost.
            let mut cursor = format!("{now_ms}-0");
            let n = q.replay.unwrap_or(DEFAULT_REPLAY).min(MAX_REPLAY);
            if n > 0 {
                let mut conn = s.redis.clone();
                let reply: redis::streams::StreamRangeReply = redis::cmd("XREVRANGE")
                    .arg(&key)
                    .arg("+")
                    .arg("-")
                    .arg("COUNT")
                    .arg(n)
                    .query_async(&mut conn)
                    .await
                    .map_err(redis_err)?;
                let mut newest = None::<String>;
                for id in reply.ids.iter().rev() {
                    if let Some((sid, data)) = entry_data(id) {
                        if newest
                            .as_deref()
                            .is_none_or(|cur| stream_id_newer(&sid, cur))
                        {
                            newest = Some(sid.clone());
                        }
                        if !expired(&data, now_ms) && wanted(&data, lang.as_deref()) {
                            buffered.push_back(event(&sid, &data));
                        }
                    }
                }
                if let Some(newest) = newest
                    && stream_id_newer(&newest, &cursor)
                {
                    cursor = newest;
                }
            }
            cursor
        }
    };

    let tail = Tail {
        state: s,
        room,
        user,
        key,
        cursor,
        lang,
        buffered,
        polls: 0,
    };
    let stream = futures::stream::unfold(tail, |mut t| async move {
        loop {
            if let Some(ev) = t.buffered.pop_front() {
                return Some((Ok(ev), t));
            }
            tokio::time::sleep(POLL).await;
            t.polls += 1;
            if t.polls.is_multiple_of(ACCESS_RECHECK_POLLS)
                && ensure_can_view(&t.state, t.room, t.user).await.is_err()
            {
                return None; // kicked or removed: close the stream
            }
            if let Err(e) = t.poll().await {
                tracing::warn!(room = %t.room, error = %e, "suggestion stream poll failed");
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FollowUpDto {
    suggestion_id: String,
    segment_id: String,
    category: String,
    owner: Option<String>,
    task: String,
    due: Option<String>,
    source_segment_ids: Vec<String>,
    confidence: f32,
    language: String,
    timestamp_ms: i64,
}

impl From<FollowUpEvent> for FollowUpDto {
    fn from(e: FollowUpEvent) -> Self {
        Self {
            suggestion_id: e.suggestion_id,
            segment_id: e.segment_id,
            category: e.category,
            owner: e.follow_up.owner,
            task: e.follow_up.task,
            due: e.follow_up.due,
            source_segment_ids: e.follow_up.source_segment_ids,
            confidence: e.confidence,
            language: e.language,
            timestamp_ms: e.timestamp_ms,
        }
    }
}

async fn followups(
    State(s): State<SuggestionsState>,
    auth: AuthUser,
    Path(room): Path<RoomId>,
) -> AppResult<Json<Vec<FollowUpDto>>> {
    ensure_can_view(&s, room, auth.user_id()).await?;
    let mut conn = s.redis.clone();
    let reply: redis::streams::StreamRangeReply = redis::cmd("XREVRANGE")
        .arg(ASSISTANT_FOLLOWUPS)
        .arg("+")
        .arg("-")
        .arg("COUNT")
        .arg(FOLLOWUP_SCAN)
        .query_async(&mut conn)
        .await
        .map_err(redis_err)?;
    let mut out: Vec<FollowUpDto> = reply
        .ids
        .iter()
        .filter_map(entry_data)
        .filter_map(|(_, v)| serde_json::from_value::<FollowUpEvent>(v).ok())
        .filter(|e| e.meeting_id == room)
        .map(Into::into)
        .collect();
    out.reverse(); // oldest first
    Ok(Json(out))
}
