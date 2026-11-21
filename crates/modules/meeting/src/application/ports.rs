//! Ports: what the meeting module needs from the outside, plus its persistence port.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use warptalk_kernel::{AppResult, MeetingRoomId, RoomId, UserId, WorkspaceId};

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

#[async_trait]
pub trait RoomDirectory: Send + Sync {
    /// `None` when the translation room does not exist.
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomInfo>>;
    async fn participant(&self, room: RoomId, user: UserId) -> AppResult<Option<ParticipantInfo>>;
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
}

impl InMemoryRoomDirectory {
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
}

// ---------------------------------------------------------------- events

pub const EVENT_END_ROOM: &str = "meeting.end_room";
pub const EVENT_BILLING_STOP: &str = "meeting.billing.stop";

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
    async fn claim_host(&self, id: MeetingRoomId, host: UserId, now: DateTime<Utc>)
    -> AppResult<()>;
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
}
