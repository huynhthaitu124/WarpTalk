//! Side effects of room events that belong to other planes, applied by decorating the room
//! module's event publisher (the room module itself stays unaware of Redis keys and LiveKit):
//!
//! - B-08: `RoomEnded` also publishes the `__MEETING_END__` marker on `stt:results` (global and
//!   per-meeting copy), which triggers transcript finalization and suggestion cleanup.
//! - B-10: `Kick` / `ParticipantRejected` also remove the participant from the LiveKit room.
//! - B-12: `translationRoom:{id}:ai_policy` (JSON) and `translationRoom:{id}:languages` (hash of
//!   participant id -> listen language) are written on room start, on every route publication
//!   and, for `ai_policy`, when the workspace settings change; both are deleted on room end.
//!
//! All effects are best effort: a failure is logged and the original event still goes out.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde_json::json;
use warptalk_contracts::events::{AudioRoutesSnapshot, RoomCommand, SttResult};
use warptalk_contracts::streams::{self, STT_RESULTS};
use warptalk_kernel::{AppError, AppResult, RoomId, WorkspaceId};
use warptalk_livekit::RoomService;
use warptalk_room::RoomQueries;
use warptalk_room::application::ports::RoomEventPublisher;
use warptalk_workspace::{WorkspaceQueries, WorkspaceSettingsObserver};

// TODO move to contracts (key builders and the stream bound live here until then).
pub fn ai_policy_key(room: RoomId) -> String {
    format!("translationRoom:{room}:ai_policy")
}

pub fn languages_key(room: RoomId) -> String {
    format!("translationRoom:{room}:languages")
}

/// Lifetime of the per-room keys (same as `translationRoom:{id}:workspace`).
const ROOM_KEYS_TTL_SECS: u64 = 172_800;
/// Approximate bound of the `stt:results` streams when the marker is appended.
const STT_STREAM_MAXLEN: usize = 100_000;
/// LiveKit identity suffixes of secondary listener connections (see the meeting join `clientTag`).
const LISTENER_SUFFIXES: [&str; 2] = ["listener", "bridge-listener"];

fn bus_err(e: impl std::fmt::Display) -> AppError {
    AppError::unavailable(format!("redis unavailable: {e}"))
}

/// Shared handles filled after the modules exist (the decorator is built first).
#[derive(Default, Clone)]
pub struct RoomEffectsHandles {
    pub rooms: Arc<OnceLock<RoomQueries>>,
    pub workspaces: Arc<OnceLock<WorkspaceQueries>>,
}

pub struct RoomEffectsPublisher {
    inner: Arc<dyn RoomEventPublisher>,
    conn: ConnectionManager,
    room_service: Arc<dyn RoomService>,
    handles: RoomEffectsHandles,
}

impl RoomEffectsPublisher {
    pub fn new(
        inner: Arc<dyn RoomEventPublisher>,
        conn: ConnectionManager,
        room_service: Arc<dyn RoomService>,
        handles: RoomEffectsHandles,
    ) -> Self {
        Self {
            inner,
            conn,
            room_service,
            handles,
        }
    }

    /// Write `ai_policy` from the room's workspace flags.
    async fn write_policy(&self, room: RoomId) {
        write_ai_policy(&self.conn, &self.handles, room).await;
    }

    async fn write_languages(&self, room: RoomId, snapshot: &AudioRoutesSnapshot) {
        let mut conn = self.conn.clone();
        let key = languages_key(room);
        let mut pipe = redis::pipe();
        pipe.atomic().cmd("DEL").arg(&key).ignore();
        let mut any = false;
        for r in &snapshot.routes {
            pipe.cmd("HSET")
                .arg(&key)
                .arg(r.target_participant_id.to_string())
                .arg(&r.target_language)
                .ignore();
            any = true;
        }
        if any {
            pipe.cmd("EXPIRE")
                .arg(&key)
                .arg(ROOM_KEYS_TTL_SECS)
                .ignore();
        }
        if let Err(e) = pipe.query_async::<()>(&mut conn).await {
            tracing::warn!(%room, error = %e, "could not write the room's reader languages");
        }
    }

    async fn on_room_ended(&self, room: RoomId) {
        let mut conn = self.conn.clone();
        let now_ms = chrono::Utc::now().timestamp_millis();
        // One marker per room end; a replayed End writes an identical id.
        let marker = SttResult::meeting_end(format!("{room}-meeting-end"), room, now_ms);
        let copy = streams::per_meeting(STT_RESULTS, room);
        for stream in [STT_RESULTS, copy.as_str()] {
            if let Err(e) =
                warptalk_bus::xadd_json(&mut conn, stream, &marker, STT_STREAM_MAXLEN).await
            {
                tracing::warn!(%room, stream, error = %e, "meeting end marker not published");
            }
        }
        let del = redis::cmd("DEL")
            .arg(ai_policy_key(room))
            .arg(languages_key(room))
            .query_async::<()>(&mut conn)
            .await;
        if let Err(e) = del {
            tracing::warn!(%room, error = %e, "could not clear the room's ai keys");
        }
    }

    /// Remove every LiveKit connection of the user (main session and listener seats).
    async fn remove_from_livekit(&self, room: RoomId, user: uuid::Uuid) {
        let room_name = room.to_string();
        let mut identities = vec![user.to_string()];
        identities.extend(LISTENER_SUFFIXES.iter().map(|s| format!("{user}:{s}")));
        for identity in identities {
            match self
                .room_service
                .remove_participant(&room_name, &identity)
                .await
            {
                Ok(()) => {}
                Err(e) if e.is_not_found() => {}
                Err(e) => {
                    tracing::warn!(%room, %identity, error = %e, "livekit remove_participant failed");
                }
            }
        }
    }
}

/// `translationRoom:{room}:ai_policy` = `{"allow_external_llm", "ai_retrieval_allowed",
/// "ai_suggest_enabled"}` from the room's workspace settings (flags default as in the readers).
pub async fn write_ai_policy(conn: &ConnectionManager, handles: &RoomEffectsHandles, room: RoomId) {
    let (Some(rooms), Some(workspaces)) = (handles.rooms.get(), handles.workspaces.get()) else {
        return;
    };
    let res: AppResult<()> = async {
        let Some(ws) = rooms.room_workspace(room).await? else {
            return Ok(());
        };
        let p = workspaces.ai_policy(ws).await?;
        let body = json!({
            "allow_external_llm": p.external_llm_allowed,
            "ai_retrieval_allowed": p.ai_retrieval_allowed,
            "ai_suggest_enabled": p.ai_suggest_enabled,
        });
        let mut conn = conn.clone();
        redis::cmd("SET")
            .arg(ai_policy_key(room))
            .arg(body.to_string())
            .arg("EX")
            .arg(ROOM_KEYS_TTL_SECS)
            .query_async::<()>(&mut conn)
            .await
            .map_err(bus_err)
    }
    .await;
    if let Err(e) = res {
        tracing::warn!(%room, error = %e, "could not write the room's ai_policy");
    }
}

#[async_trait]
impl RoomEventPublisher for RoomEffectsPublisher {
    async fn publish_command(&self, cmd: &RoomCommand) -> AppResult<()> {
        if let RoomCommand::RoomStarted { state, .. } = cmd {
            self.write_policy(state.room_id).await;
        }
        let out = self.inner.publish_command(cmd).await;
        match cmd {
            RoomCommand::RoomEnded { room_id } => self.on_room_ended(*room_id).await,
            RoomCommand::Kick {
                room_id,
                user_id: Some(user),
            }
            | RoomCommand::ParticipantRejected {
                room_id,
                user_id: Some(user),
            } => self.remove_from_livekit(*room_id, *user).await,
            _ => {}
        }
        out
    }

    async fn publish_routes(&self, room: RoomId, snapshot: &AudioRoutesSnapshot) -> AppResult<()> {
        if snapshot.room_status == "IN_PROGRESS" {
            self.write_policy(room).await;
            self.write_languages(room, snapshot).await;
        }
        self.inner.publish_routes(room, snapshot).await
    }
}

/// Workspace settings changed: refresh `ai_policy` of the workspace's live rooms.
pub struct RoomAiPolicyRefresher {
    pub conn: ConnectionManager,
    pub handles: RoomEffectsHandles,
}

#[async_trait]
impl WorkspaceSettingsObserver for RoomAiPolicyRefresher {
    async fn settings_changed(&self, ws: WorkspaceId) {
        let Some(rooms) = self.handles.rooms.get() else {
            return;
        };
        match rooms.in_progress_rooms(ws).await {
            Ok(ids) => {
                for room in ids {
                    write_ai_policy(&self.conn, &self.handles, room).await;
                }
            }
            Err(e) => {
                tracing::warn!(%ws, error = %e, "could not list live rooms for the ai policy refresh")
            }
        }
    }
}
