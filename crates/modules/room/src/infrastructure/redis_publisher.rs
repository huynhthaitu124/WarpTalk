//! Redis implementation of [`RoomEventPublisher`].

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde_json::json;
use warptalk_bus::publish_json;
use warptalk_contracts::channels::{TRANSLATION_ROOM_COMMANDS, room_events};
use warptalk_contracts::events::{AudioRoutesSnapshot, RoomCommand};
use warptalk_contracts::keys::audio_routes;
use warptalk_kernel::{AppError, AppResult, RoomId};

use crate::application::ports::RoomEventPublisher;

/// TTL of the cached route snapshot: 12 h (spec: 10 §3.7).
pub const ROUTES_TTL_SECS: u64 = 43_200;

pub struct RedisRoomEventPublisher {
    conn: ConnectionManager,
}

impl RedisRoomEventPublisher {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

fn unavailable(e: impl std::fmt::Display) -> AppError {
    AppError::unavailable(format!("event bus unavailable: {e}"))
}

#[async_trait]
impl RoomEventPublisher for RedisRoomEventPublisher {
    async fn publish_command(&self, cmd: &RoomCommand) -> AppResult<()> {
        let mut conn = self.conn.clone();
        publish_json(&mut conn, TRANSLATION_ROOM_COMMANDS, cmd)
            .await
            .map(|_| ())
            .map_err(unavailable)
    }

    async fn publish_routes(&self, room: RoomId, snapshot: &AudioRoutesSnapshot) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let body =
            serde_json::to_string(snapshot).map_err(|e| AppError::internal(e.to_string()))?;
        redis::cmd("SET")
            .arg(audio_routes(room))
            .arg(body)
            .arg("EX")
            .arg(ROUTES_TTL_SECS)
            .query_async::<()>(&mut conn)
            .await
            .map_err(unavailable)?;
        publish_json(
            &mut conn,
            &room_events(room),
            &json!({ "type": "AUDIO_ROUTES_UPDATED", "data": snapshot }),
        )
        .await
        .map(|_| ())
        .map_err(unavailable)
    }
}
