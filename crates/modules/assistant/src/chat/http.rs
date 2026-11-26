//! REST + SSE of the meeting assistant (paths relative to `/api/v1`, camelCase responses).
//!
//! - `POST /meetings/{room_id}/ask` `{question, scope?, since_ms?}`
//! - `POST /meetings/{room_id}/recap` `{last_minutes?, language?}`
//! - `GET  /meetings/{room_id}/ask/history?limit=&before=&kind=`
//!
//! `ask` and `recap` stream by default (`text/event-stream`: `delta`, `citations`, `done`,
//! `error`); `?stream=false` returns one JSON document. Access: whoever can view the room.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, RoomId, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};

use super::model::{AskRequest, ChatAnswer, ChatEvent, Citation, HistoryItem, RecapRequest};
use super::service::{ChatService, Prepared};
use crate::summary::RoomAccess;

/// Everything the chat routes need.
#[derive(Clone)]
pub struct ChatApi {
    pub service: Arc<ChatService>,
    pub access: Arc<dyn RoomAccess>,
}

#[derive(Clone)]
struct State_ {
    api: ChatApi,
    jwt: JwtKeys,
}

impl FromRef<State_> for JwtKeys {
    fn from_ref(s: &State_) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(api: ChatApi, jwt: JwtKeys) -> Router {
    Router::new()
        .route("/meetings/{room_id}/ask", post(ask))
        .route("/meetings/{room_id}/ask/history", get(history))
        .route("/meetings/{room_id}/recap", post(recap))
        .with_state(State_ { api, jwt })
}

#[derive(Deserialize)]
struct StreamQuery {
    /// Default true.
    stream: Option<bool>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<u32>,
    /// Message id: only older messages.
    before: Option<Uuid>,
    /// `ask` | `recap`.
    kind: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryResponse {
    items: Vec<HistoryItem>,
    /// Pass as `before` for the next older page; absent on the last page.
    next_before: Option<Uuid>,
}

async fn can_view(s: &State_, room: RoomId, auth: &AuthUser) -> AppResult<WorkspaceId> {
    let info = s
        .api
        .access
        .room_access(room, auth.user_id())
        .await?
        .ok_or_else(|| AppError::not_found("meeting not found"))?;
    if !info.can_view() {
        return Err(AppError::forbidden(
            "not allowed to use the assistant in this meeting",
        ));
    }
    Ok(info.workspace_id)
}

async fn ask(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
    Query(q): Query<StreamQuery>,
    Json(req): Json<AskRequest>,
) -> AppResult<Response> {
    let room = RoomId::from_uuid(room);
    let ws = can_view(&s, room, &auth).await?;
    let prepared = s.api.service.ask(ws, room, auth.user_id(), req).await?;
    respond(&s, prepared, q.stream.unwrap_or(true)).await
}

async fn recap(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
    Query(q): Query<StreamQuery>,
    body: Option<Json<RecapRequest>>,
) -> AppResult<Response> {
    let room = RoomId::from_uuid(room);
    let ws = can_view(&s, room, &auth).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let prepared = s.api.service.recap(ws, room, auth.user_id(), req).await?;
    respond(&s, prepared, q.stream.unwrap_or(true)).await
}

async fn respond(s: &State_, prepared: Prepared, stream: bool) -> AppResult<Response> {
    let mut rx = s.api.service.execute(prepared);
    if !stream {
        let mut answer = String::new();
        let mut citations: Vec<Citation> = Vec::new();
        while let Some(ev) = rx.recv().await {
            match ev {
                ChatEvent::Delta(t) => answer.push_str(&t),
                ChatEvent::Citations(c) => citations = c,
                ChatEvent::Done {
                    model,
                    message_id,
                    cached,
                } => {
                    return Ok(Json(ChatAnswer {
                        answer,
                        citations,
                        model,
                        message_id,
                        cached,
                    })
                    .into_response());
                }
                ChatEvent::Error { code, message } => {
                    let status = if code == super::model::codes::LLM_UNAVAILABLE {
                        503
                    } else {
                        502
                    };
                    return Err(AppError::coded(status, code, message));
                }
            }
        }
        return Err(AppError::internal("assistant stopped without an answer"));
    }
    let events = futures::stream::unfold(rx, |mut rx| async move {
        let ev = rx.recv().await?;
        Some((Ok::<_, Infallible>(to_sse(&ev)), rx))
    });
    Ok(Sse::new(events)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}

fn to_sse(ev: &ChatEvent) -> Event {
    let (name, data) = match ev {
        ChatEvent::Delta(t) => ("delta", json!({ "text": t })),
        ChatEvent::Citations(c) => ("citations", json!({ "citations": c })),
        ChatEvent::Done {
            model,
            message_id,
            cached,
        } => (
            "done",
            json!({ "model": model, "messageId": message_id, "cached": cached }),
        ),
        ChatEvent::Error { code, message } => {
            ("error", json!({ "code": code, "message": message }))
        }
    };
    Event::default().event(name).data(data.to_string())
}

async fn history(
    State(s): State<State_>,
    auth: AuthUser,
    Path(room): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<HistoryResponse>> {
    let room = RoomId::from_uuid(room);
    let ws = can_view(&s, room, &auth).await?;
    let cfg = s.api.service.config(ws, room).await;
    let limit = q.limit.unwrap_or(50).clamp(1, cfg.history_max_page);
    let kind = match q.kind.as_deref() {
        None => None,
        Some(k @ ("ask" | "recap")) => Some(k),
        Some(_) => {
            return Err(AppError::validation(
                "INVALID_REQUEST",
                "kind must be `ask` or `recap`",
            ));
        }
    };
    let items = s
        .api
        .service
        .repo()
        .history(ws, room, auth.user_id(), kind, q.before, limit)
        .await?;
    let next_before = (items.len() as u32 == limit)
        .then(|| items.first().map(|i| i.id))
        .flatten();
    Ok(Json(HistoryResponse { items, next_before }))
}
