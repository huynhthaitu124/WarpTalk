//! WarpBot REST routes and the live event stream (SSE) of a conversation.
//!
//! The stream carries the hub events the web client handles (`AssistantMessageStarted`,
//! `AssistantMessageChunk`, ...): SSE event name = hub event name, data = its JSON payload,
//! id = bus cursor so a reconnect with `Last-Event-ID` resumes without gaps or repeats.
//! Auth comes from the `Authorization` header (the client reads the stream with `fetch`), never
//! from the URL.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys};

use crate::application::chat::SEMANTIC_SEARCH;
use crate::application::{ChatService, PageContext, SendCommand};
use crate::domain::chat::{ChatMessage, Conversation, MessageRole};

const POLL: Duration = Duration::from_millis(250);
/// Membership is re-checked every this many polls (~30 s).
const ACCESS_RECHECK_POLLS: u32 = 120;

#[derive(Clone)]
pub struct ChatState {
    pub chat: ChatService,
    pub jwt: JwtKeys,
}

impl FromRef<ChatState> for JwtKeys {
    fn from_ref(s: &ChatState) -> JwtKeys {
        s.jwt.clone()
    }
}

/// Paths relative to `/api/v1`.
pub fn router(state: ChatState) -> Router {
    Router::new()
        .route(
            "/assistant/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/assistant/conversations/{id}",
            get(get_conversation).delete(archive_conversation),
        )
        .route("/assistant/conversations/{id}/messages", post(send_message))
        .route(
            "/assistant/conversations/{id}/messages/{mid}/stop",
            post(stop_reply),
        )
        .route("/assistant/conversations/{id}/events", get(events))
        .route("/assistant/skills", get(skills))
        .route("/assistant/tools", get(tools))
        .with_state(state)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDto {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub is_archived: bool,
    pub scope: &'static str,
}

impl From<&Conversation> for ConversationDto {
    fn from(c: &Conversation) -> Self {
        Self {
            id: c.id,
            title: c.title.clone(),
            created_at: c.created_at,
            last_message_at: c.last_message_at,
            is_archived: c.is_archived,
            scope: "workspace",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: &'static str,
    pub content: String,
    pub status: &'static str,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub sources_json: Option<String>,
}

impl From<&ChatMessage> for MessageDto {
    fn from(m: &ChatMessage) -> Self {
        Self {
            id: m.id,
            conversation_id: m.conversation_id,
            role: m.role.as_str(),
            content: m.content.clone(),
            status: m.status.as_str(),
            created_at: m.created_at,
            completed_at: m.completed_at,
            sources_json: m.sources_json.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetailDto {
    #[serde(flatten)]
    pub conversation: ConversationDto,
    pub messages: Vec<MessageDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceQuery {
    pub workspace_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct SeedDto {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationRequest {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub seed_messages: Vec<SeedDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageContextDto {
    #[serde(default)]
    pub page_type: String,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub snapshot: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub page_context: Option<PageContextDto>,
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResponse {
    pub message_id: Uuid,
    pub assistant_message_id: Uuid,
}

async fn list_conversations(
    State(s): State<ChatState>,
    auth: AuthUser,
    Query(q): Query<WorkspaceQuery>,
) -> AppResult<Json<Vec<ConversationDto>>> {
    let list = s
        .chat
        .list(auth.user_id(), WorkspaceId::from_uuid(q.workspace_id))
        .await?;
    Ok(Json(list.iter().map(ConversationDto::from).collect()))
}

async fn create_conversation(
    State(s): State<ChatState>,
    auth: AuthUser,
    Json(req): Json<CreateConversationRequest>,
) -> AppResult<(StatusCode, Json<ConversationDto>)> {
    let seeds = req
        .seed_messages
        .into_iter()
        .map(|m| {
            MessageRole::parse(m.role.trim())
                .map(|r| (r, m.content))
                .ok_or_else(|| {
                    AppError::validation(
                        "INVALID_SEED_ROLE",
                        "seed messages must be user or assistant",
                    )
                })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let conv = s
        .chat
        .create(
            auth.user_id(),
            WorkspaceId::from_uuid(req.workspace_id),
            req.title,
            seeds,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(ConversationDto::from(&conv))))
}

async fn get_conversation(
    State(s): State<ChatState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConversationDetailDto>> {
    let (conv, messages) = s.chat.detail(auth.user_id(), id).await?;
    Ok(Json(ConversationDetailDto {
        conversation: ConversationDto::from(&conv),
        messages: messages.iter().map(MessageDto::from).collect(),
    }))
}

async fn archive_conversation(
    State(s): State<ChatState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    s.chat.archive(auth.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn send_message(
    State(s): State<ChatState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> AppResult<(StatusCode, Json<SendMessageResponse>)> {
    let page_context = req.page_context.map(|p| PageContext {
        page_type: p.page_type,
        entity_id: p.entity_id,
        snapshot: p
            .snapshot
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some((k, s)),
                Value::Number(n) => Some((k, n.to_string())),
                Value::Bool(b) => Some((k, b.to_string())),
                _ => None,
            })
            .collect(),
    });
    let ack = s
        .chat
        .send(
            auth.user_id(),
            id,
            SendCommand {
                content: req.content,
                page_context,
                attachments: req.attachments.map_or(0, |a| a.len()),
            },
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SendMessageResponse {
            message_id: ack.message_id,
            assistant_message_id: ack.assistant_message_id,
        }),
    ))
}

async fn stop_reply(
    State(s): State<ChatState>,
    auth: AuthUser,
    Path((id, mid)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    s.chat.stop(auth.user_id(), id, mid).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// WarpBot has no slash skills yet.
async fn skills(_auth: AuthUser) -> Json<Vec<Value>> {
    Json(Vec::new())
}

async fn tools(State(s): State<ChatState>, _auth: AuthUser) -> Json<Value> {
    let built_in: Vec<Value> = if s.chat.search_available() {
        vec![json!({
            "name": SEMANTIC_SEARCH,
            "category": "knowledge",
            "effect": "read",
            "audience": "member",
            "description": "Search the workspace's documents, meeting transcripts, meeting notes \
                            and glossary by meaning.",
        })]
    } else {
        Vec::new()
    };
    Json(json!({
        "manifestAvailable": true,
        "manifestGeneratedAt": Value::Null,
        "builtIn": built_in,
        "webSearch": { "state": "unavailable" },
        "plugins": [],
    }))
}

fn valid_cursor(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

struct Tail {
    chat: ChatService,
    user: UserId,
    conversation: Uuid,
    cursor: String,
    buffered: VecDeque<Event>,
    polls: u32,
}

impl Tail {
    async fn poll(&mut self) -> AppResult<()> {
        let batch = self
            .chat
            .read_events(self.conversation, &self.cursor)
            .await?;
        for (id, ev) in batch {
            self.buffered.push_back(
                Event::default()
                    .event(ev.name)
                    .id(id.clone())
                    .data(ev.data.to_string()),
            );
            self.cursor = id;
        }
        Ok(())
    }
}

async fn events(
    State(s): State<ChatState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let user = auth.user_id();
    s.chat.ensure_can_read(user, id).await?;
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| valid_cursor(v))
        .map(str::to_owned);
    let cursor = match resume {
        Some(c) => c,
        None => s.chat.events_cursor(id).await?,
    };
    let mut buffered = VecDeque::new();
    // Tells the client the stream is live; carries no id so it never moves the resume cursor.
    buffered.push_back(Event::default().event("ready").data("{}"));
    let tail = Tail {
        chat: s.chat.clone(),
        user,
        conversation: id,
        cursor,
        buffered,
        polls: 0,
    };
    let stream = futures::stream::unfold(tail, |mut t| async move {
        loop {
            if let Some(ev) = t.buffered.pop_front() {
                return Some((Ok(ev), t));
            }
            tokio::time::sleep(POLL).await;
            t.polls = t.polls.wrapping_add(1);
            if t.polls.is_multiple_of(ACCESS_RECHECK_POLLS)
                && t.chat
                    .ensure_can_read(t.user, t.conversation)
                    .await
                    .is_err()
            {
                return None; // archived or no longer a member: close the stream
            }
            if let Err(e) = t.poll().await {
                tracing::warn!(conversation = %t.conversation, error = %e, "assistant stream poll failed");
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
