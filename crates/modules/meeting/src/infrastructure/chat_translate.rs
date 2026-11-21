//! Chat translation consumer: `translate:chat_requests` -> `meeting_chat_translations`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage, publish_json};
use warptalk_contracts::events::DomainEventEnvelope;
use warptalk_kernel::{AppError, AppResult, Clock, MeetingRoomId, RoomId, WorkspaceId};

use crate::application::ports::{
    CHAT_TRANSLATE_REQUESTS_STREAM, ChatEventSink, ChatTargetLanguages, ChatTranslator,
    EVENT_CHAT_TRANSLATE, MeetingRepo, NewChatTranslation, TranslateError,
};
use crate::domain::{detect_chat_language, primary_language};
use crate::infrastructure::pg_repo::PgMeetingRepo;

pub const CHAT_TRANSLATE_GROUP: &str = "meeting-chat-translate";

/// Translation is best-effort: give up after three deliveries.
pub fn chat_translate_consumer_config() -> ConsumerConfig {
    let mut c = ConsumerConfig::new(CHAT_TRANSLATE_REQUESTS_STREAM, CHAT_TRANSLATE_GROUP);
    c.max_deliveries = 3;
    c
}

/// Pub/sub channel the realtime hub forwards to the room's clients.
pub fn chat_channel(room: RoomId) -> String {
    format!("meeting:{room}:chat")
}

/// Redis pub/sub implementation of [`ChatEventSink`].
pub struct RedisChatEventSink {
    conn: ConnectionManager,
}

impl RedisChatEventSink {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl ChatEventSink for RedisChatEventSink {
    async fn publish(&self, channel: &str, payload: Value) -> AppResult<()> {
        let mut conn = self.conn.clone();
        publish_json(&mut conn, channel, &payload)
            .await
            .map(|_| ())
            .map_err(|e| AppError::unavailable(format!("event bus unavailable: {e}")))
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    message_id: Uuid,
    meeting_room_id: Uuid,
    translation_room_id: Uuid,
    workspace_id: Uuid,
}

pub struct ChatTranslateHandler {
    repo: Arc<dyn MeetingRepo>,
    targets: Arc<dyn ChatTargetLanguages>,
    translator: Arc<dyn ChatTranslator>,
    sink: Arc<dyn ChatEventSink>,
    clock: Arc<dyn Clock>,
}

impl ChatTranslateHandler {
    pub fn new(
        pool: PgPool,
        targets: Arc<dyn ChatTargetLanguages>,
        translator: Arc<dyn ChatTranslator>,
        sink: Arc<dyn ChatEventSink>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo: Arc::new(PgMeetingRepo::new(pool)),
            targets,
            translator,
            sink,
            clock,
        }
    }

    async fn run(&self, req: Request) -> Result<(), HandlerError> {
        let retry = |e: AppError| HandlerError::Retry(e.to_string());
        let room = MeetingRoomId::from_uuid(req.meeting_room_id);
        let translation_room = RoomId::from_uuid(req.translation_room_id);
        let workspace = WorkspaceId::from_uuid(req.workspace_id);

        let Some(msg) = self
            .repo
            .chat_message(room, req.message_id)
            .await
            .map_err(retry)?
        else {
            return Ok(());
        };
        if msg.hidden {
            return Ok(());
        }

        // HEURISTIC detection (script based), recorded only while the language is unknown.
        let source = detect_chat_language(&msg.content);
        if msg.original_language == "und" {
            self.repo
                .set_chat_original_language(room, msg.id, source)
                .await
                .map_err(retry)?;
        }
        let source_lang = if msg.original_language == "und" {
            source.to_owned()
        } else {
            msg.original_language.clone()
        };
        let source_primary = primary_language(&source_lang);

        let mut targets: Vec<String> = Vec::new();
        for t in self
            .targets
            .for_room(translation_room)
            .await
            .map_err(retry)?
        {
            let p = primary_language(&t);
            if p.is_empty()
                || p == source_primary
                || targets.iter().any(|x| primary_language(x) == p)
            {
                continue;
            }
            targets.push(t);
        }
        if targets.is_empty() {
            return Ok(());
        }

        let mut all: BTreeMap<String, String> =
            self.repo.chat_translations(msg.id).await.map_err(retry)?;
        let mut added = false;
        let mut transient: Option<String> = None;
        for target in targets {
            if all.contains_key(&target) {
                continue;
            }
            match self
                .translator
                .translate(workspace, &msg.content, &source_lang, &target)
                .await
            {
                Ok(t) => {
                    let inserted = self
                        .repo
                        .insert_chat_translation(NewChatTranslation {
                            message_id: msg.id,
                            meeting_room_id: room,
                            source_language: &source_lang,
                            target_language: &target,
                            translated_text: &t.text,
                            model_used: t.model.as_deref(),
                            now: self.clock.now(),
                        })
                        .await
                        .map_err(retry)?;
                    added |= inserted;
                    all.insert(target, t.text);
                }
                Err(TranslateError::Transient(e)) => transient = Some(e),
                Err(TranslateError::Permanent(e)) => {
                    tracing::warn!(message = %msg.id, %target, error = %e, "chat translation skipped");
                }
            }
        }
        if added {
            let payload = json!({
                "type": "chat.translated",
                "messageId": msg.id,
                "translations": all,
            });
            if let Err(e) = self
                .sink
                .publish(&chat_channel(translation_room), payload)
                .await
            {
                tracing::warn!(message = %msg.id, error = %e, "chat.translated not published");
            }
        }
        match transient {
            Some(e) => Err(HandlerError::Retry(e)),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl StreamHandler for ChatTranslateHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let env: DomainEventEnvelope = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("undecodable chat request: {e}")))?;
        if env.event_type != EVENT_CHAT_TRANSLATE {
            return Ok(());
        }
        let req: Request = serde_json::from_value(env.payload)
            .map_err(|e| HandlerError::Fatal(format!("undecodable chat request payload: {e}")))?;
        self.run(req).await
    }
}
