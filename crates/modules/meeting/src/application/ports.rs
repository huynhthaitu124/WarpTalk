//! Ports: what the meeting module needs from the outside, plus its persistence port.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, MeetingRoomId, RoomId, UserId, WorkspaceId};

use crate::domain::TrackChange;

/// Re-exported provider port (LiveKit Twirp `RoomService`).
pub use warptalk_livekit::RoomService;

// ---------------------------------------------------------------- room directory

/// Translation-room facts the join gate needs (api wires the room module's queries).
#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub workspace_id: WorkspaceId,
    /// Effective host: `COALESCE(active_host_id, host_id)`.
    pub host_id: UserId,
    /// Translation-room status string (e.g. `IN_PROGRESS`, `ENDED`).
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ParticipantInfo {
    /// Translation-room participant status (`WAITING`, `CONNECTED`, ...).
    pub status: String,
    pub display_name: String,
}

/// Bridge facts of an `EXTERNAL_BRIDGE` translation room.
#[derive(Debug, Clone)]
pub struct BridgeInfo {
    /// `google_meet | zoom | teams`.
    pub provider: String,
    pub capturer_user_id: Option<UserId>,
    /// The capturer lease (or its audio) is stale.
    pub capturer_stale: bool,
}

#[async_trait]
pub trait RoomDirectory: Send + Sync {
    /// `Some` only for `EXTERNAL_BRIDGE` rooms. The default (`None`) fails closed: without an
    /// adapter that answers, no bridge token is ever issued.
    async fn bridge(&self, _room: RoomId) -> AppResult<Option<BridgeInfo>> {
        Ok(None)
    }
    /// `None` when the translation room does not exist.
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomInfo>>;
    async fn participant(&self, room: RoomId, user: UserId) -> AppResult<Option<ParticipantInfo>>;
    /// End the translation room as `actor` (the host). Idempotent for an already ended room.
    /// The default fails closed (`503`): adapters that cannot end rooms must say so.
    async fn end_room(&self, _room: RoomId, _actor: UserId) -> AppResult<()> {
        Err(AppError::unavailable("room directory cannot end rooms"))
    }
}

/// Knows no rooms.
pub struct NoopRoomDirectory;

#[async_trait]
impl RoomDirectory for NoopRoomDirectory {
    async fn room(&self, _: RoomId) -> AppResult<Option<RoomInfo>> {
        Ok(None)
    }
    async fn participant(&self, _: RoomId, _: UserId) -> AppResult<Option<ParticipantInfo>> {
        Ok(None)
    }
}

/// Settable in-memory directory for tests.
#[derive(Default)]
pub struct InMemoryRoomDirectory {
    rooms: Mutex<HashMap<RoomId, RoomInfo>>,
    participants: Mutex<HashMap<(RoomId, UserId), ParticipantInfo>>,
    bridges: Mutex<HashMap<RoomId, BridgeInfo>>,
    ended: Mutex<Vec<(RoomId, UserId)>>,
}

impl InMemoryRoomDirectory {
    /// `(room, actor)` of every `end_room` call, in order.
    pub fn ended_rooms(&self) -> Vec<(RoomId, UserId)> {
        self.ended.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Mark `room` as an EXTERNAL_BRIDGE room.
    pub fn set_bridge(&self, room: RoomId, info: BridgeInfo) {
        self.bridges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(room, info);
    }

    pub fn set_room(&self, room: RoomId, info: RoomInfo) {
        self.rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(room, info);
    }

    pub fn set_participant(&self, room: RoomId, user: UserId, status: &str, display_name: &str) {
        self.participants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (room, user),
                ParticipantInfo {
                    status: status.to_owned(),
                    display_name: display_name.to_owned(),
                },
            );
    }
}

#[async_trait]
impl RoomDirectory for InMemoryRoomDirectory {
    async fn bridge(&self, room: RoomId) -> AppResult<Option<BridgeInfo>> {
        Ok(self
            .bridges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&room)
            .cloned())
    }
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomInfo>> {
        Ok(self
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&room)
            .cloned())
    }
    async fn participant(&self, room: RoomId, user: UserId) -> AppResult<Option<ParticipantInfo>> {
        Ok(self
            .participants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(room, user))
            .cloned())
    }
    async fn end_room(&self, room: RoomId, actor: UserId) -> AppResult<()> {
        self.ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((room, actor));
        if let Some(r) = self
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&room)
        {
            r.status = "ENDED".to_owned();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- events

pub const EVENT_END_ROOM: &str = "meeting.end_room";
pub const EVENT_BILLING_STOP: &str = "meeting.billing.stop";
/// A new chat message that the translation worker should translate (best-effort, additive).
pub const EVENT_CHAT_TRANSLATE: &str = "meeting.chat.translate_request";
/// A host hid a chat message (the hub broadcasts `ChatMessageHidden` from it).
pub const EVENT_CHAT_HIDDEN: &str = "meeting.chat.hidden";
/// Redis stream that `EVENT_CHAT_TRANSLATE` is written to by the api wiring.
// spec: 06 §3 (`PublishChatForTranslationAsync`); stream name is not given by the spec, so it is
// defined here next to the event until `warptalk-contracts` names it.
pub const CHAT_TRANSLATE_REQUESTS_STREAM: &str = "translate:chat_requests";

#[derive(Debug, Clone, PartialEq)]
pub struct MeetingEvent {
    pub event_type: &'static str,
    pub payload: Value,
}

#[async_trait]
pub trait MeetingEventPublisher: Send + Sync {
    async fn publish(&self, event: MeetingEvent) -> AppResult<()>;
}

pub struct NoopMeetingEventPublisher;

#[async_trait]
impl MeetingEventPublisher for NoopMeetingEventPublisher {
    async fn publish(&self, _: MeetingEvent) -> AppResult<()> {
        Ok(())
    }
}

/// Records published events.
#[derive(Default)]
pub struct InMemoryMeetingEventPublisher {
    events: Mutex<Vec<MeetingEvent>>,
}

impl InMemoryMeetingEventPublisher {
    pub fn events(&self) -> Vec<MeetingEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl MeetingEventPublisher for InMemoryMeetingEventPublisher {
    async fn publish(&self, event: MeetingEvent) -> AppResult<()> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
        Ok(())
    }
}

// ---------------------------------------------------------------- persistence

#[derive(Debug, Clone)]
pub struct MeetingRoom {
    pub id: MeetingRoomId,
    pub translation_room_id: RoomId,
    pub provider_room_name: String,
    pub active_host_id: Option<UserId>,
    pub status: String,
}

pub struct NewParticipant<'a> {
    pub meeting_room_id: MeetingRoomId,
    pub user_id: UserId,
    pub identity: &'a str,
    pub display_name: &'a str,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: Uuid,
    pub meeting_room_id: MeetingRoomId,
    pub sender_id: Option<UserId>,
    pub sender_name: String,
    pub content: String,
    pub original_language: String,
    pub created_at: DateTime<Utc>,
    pub hidden: bool,
    /// target language -> translated text (filled by the translation worker).
    pub translations: BTreeMap<String, String>,
}

pub struct NewChatMessage<'a> {
    pub meeting_room_id: MeetingRoomId,
    pub workspace_id: WorkspaceId,
    pub sender_id: UserId,
    pub sender_name: &'a str,
    pub content: &'a str,
    pub original_language: &'a str,
    pub client_message_id: &'a str,
    pub now: DateTime<Utc>,
}

/// A translation row to store (idempotent on message + target language).
pub struct NewChatTranslation<'a> {
    pub message_id: Uuid,
    pub meeting_room_id: MeetingRoomId,
    pub source_language: &'a str,
    pub target_language: &'a str,
    pub translated_text: &'a str,
    pub model_used: Option<&'a str>,
    pub now: DateTime<Utc>,
}

/// Persistence port (schema `meeting`), implemented by `infrastructure::PgMeetingRepo`.
#[async_trait]
pub trait MeetingRepo: Send + Sync {
    /// The current (`is_current`) session of a translation room.
    async fn find_current(&self, room: RoomId) -> AppResult<Option<MeetingRoom>>;
    async fn find_by_provider_name(&self, name: &str) -> AppResult<Option<MeetingRoom>>;
    /// Insert a new ACTIVE session; a concurrent insert for the same room is not an error.
    async fn insert_room(
        &self,
        room: RoomId,
        provider_room_name: &str,
        host: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    /// Mark ACTIVE (not for FINISHED rooms); clears a webhook-recorded close.
    async fn activate(&self, id: MeetingRoomId, now: DateTime<Utc>) -> AppResult<()>;
    async fn claim_host(
        &self,
        id: MeetingRoomId,
        host: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn upsert_participant(&self, p: NewParticipant<'_>) -> AppResult<()>;
    /// Conditional FINISHED transition: `true` only for the single winner.
    async fn try_mark_finished(&self, id: MeetingRoomId, at: DateTime<Utc>) -> AppResult<bool>;
    /// Undo a won `try_mark_finished` whose follow-up (event publish) failed.
    async fn reopen(&self, id: MeetingRoomId, now: DateTime<Utc>) -> AppResult<()>;

    /// Returns whether the event changed state (false: stale or replayed).
    async fn participant_joined(
        &self,
        id: MeetingRoomId,
        identity: &str,
        at: DateTime<Utc>,
    ) -> AppResult<bool>;
    async fn participant_left(
        &self,
        id: MeetingRoomId,
        identity: &str,
        at: DateTime<Utc>,
    ) -> AppResult<bool>;
    async fn clear_active_host_if(
        &self,
        id: MeetingRoomId,
        identity: &str,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
    async fn apply_track(
        &self,
        id: MeetingRoomId,
        identity: &str,
        track_sid: &str,
        media_type: &str,
        change: TrackChange,
        at: DateTime<Utc>,
    ) -> AppResult<()>;
    /// `room_finished`: close the session and its participants without touching `status`.
    async fn close_room(&self, id: MeetingRoomId, at: DateTime<Utc>) -> AppResult<()>;

    // ------------------------------------------------------------ chat

    /// Insert, idempotent on `(meeting room, sender, client_message_id)`: a replay returns the
    /// stored message and `false`.
    async fn insert_chat_message(&self, m: NewChatMessage<'_>) -> AppResult<(ChatMessage, bool)>;
    /// Oldest-first page ending right before `before` (a message id of the same room).
    async fn list_chat_messages(
        &self,
        room: MeetingRoomId,
        before: Option<Uuid>,
        limit: i64,
        include_hidden: bool,
    ) -> AppResult<Vec<ChatMessage>>;
    /// Hide (never erase) a message and record the moderation event once. `None`: no such
    /// message in this meeting.
    async fn hide_chat_message(
        &self,
        room: MeetingRoomId,
        message: Uuid,
        by: UserId,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<Option<ChatMessage>>;

    // ------------------------------------------------------------ chat translation

    /// One message of a meeting (hidden or not).
    async fn chat_message(
        &self,
        room: MeetingRoomId,
        message: Uuid,
    ) -> AppResult<Option<ChatMessage>>;
    /// Replace the placeholder language (`und`) of a message; a known language is kept.
    async fn set_chat_original_language(
        &self,
        room: MeetingRoomId,
        message: Uuid,
        language: &str,
    ) -> AppResult<()>;
    /// `false`: a translation for this message and language already existed (kept).
    async fn insert_chat_translation(&self, t: NewChatTranslation<'_>) -> AppResult<bool>;
    /// target language -> translated text.
    async fn chat_translations(&self, message: Uuid) -> AppResult<BTreeMap<String, String>>;
}

// ---------------------------------------------------------------- chat translation ports

/// Languages a room's chat is translated into (room source + target languages).
#[async_trait]
pub trait ChatTargetLanguages: Send + Sync {
    async fn for_room(&self, room: RoomId) -> AppResult<Vec<String>>;
}

/// Translation failure classes: only `Transient` is worth redelivering.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TranslateError {
    #[error("transient translation failure: {0}")]
    Transient(String),
    #[error("translation failed: {0}")]
    Permanent(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatTranslation {
    pub text: String,
    pub model: Option<String>,
}

#[async_trait]
pub trait ChatTranslator: Send + Sync {
    async fn translate(
        &self,
        workspace: WorkspaceId,
        text: &str,
        source_language: &str,
        target_language: &str,
    ) -> Result<ChatTranslation, TranslateError>;
}

/// Fire-and-forget pub/sub (the realtime hub fans these out to clients).
#[async_trait]
pub trait ChatEventSink: Send + Sync {
    async fn publish(&self, channel: &str, payload: Value) -> AppResult<()>;
}
