//! Consumer of `translationRoom:system_events`: records `ROUTE_DEGRADED` as the route's runtime
//! status in the room module (via its read/command facade), so `/audio-routes` can show
//! `CAPTION_ONLY` / `STANDARD_VOICE` instead of staying `PENDING` forever.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
use warptalk_contracts::events::{RouteStatus, TranslationRoomSystemEvent};
use warptalk_contracts::streams::{
    TRANSLATION_ROOM_SYSTEM_EVENTS, TRANSLATION_ROOM_SYSTEM_EVENTS_DLQ, groups,
};
use warptalk_kernel::SystemClock;
use warptalk_room::application::ports::{
    NoopRoomEventPublisher, NoopVoiceConsentDirectory, NoopWorkspacePolicy,
};
use warptalk_room::{RoomDeps, RoomModule, RoomQueries};

/// Event type written by the pipeline's TTS stage (`apps/pipeline/src/downstream.rs`).
// TODO move to contracts
pub const ROUTE_DEGRADED: &str = "ROUTE_DEGRADED";

/// Tier the route fell to (payload field `to`) as a route status. The pipeline writes the Rust
/// debug name of its tier (`TextOnly`, `Standard`); the wire status names are accepted too.
/// `Clone` (the top tier) and unknown tiers change nothing.
pub fn degraded_status(to: &str) -> Option<RouteStatus> {
    match to {
        "TextOnly" | "CAPTION_ONLY" => Some(RouteStatus::CaptionOnly),
        "Standard" | "STANDARD_VOICE" => Some(RouteStatus::StandardVoice),
        "VOICE_DELAYED" => Some(RouteStatus::VoiceDelayed),
        _ => None,
    }
}

pub struct RouteDegradedHandler {
    rooms: RoomQueries,
}

impl RouteDegradedHandler {
    pub fn new(rooms: RoomQueries) -> Self {
        Self { rooms }
    }
}

#[async_trait]
impl StreamHandler for RouteDegradedHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let ev: TranslationRoomSystemEvent = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("bad system event: {e}")))?;
        if ev.event_type != ROUTE_DEGRADED {
            return Ok(());
        }
        let (Some(route), Some(status)) = (
            ev.route_id,
            ev.payload
                .get("to")
                .and_then(|v| v.as_str())
                .and_then(degraded_status),
        ) else {
            tracing::debug!(room = %ev.room_id, "ROUTE_DEGRADED without a mappable route/status");
            return Ok(());
        };
        match self.rooms.set_route_status(ev.room_id, route, status).await {
            Ok(updated) => {
                tracing::info!(room = %ev.room_id, %route, ?status, updated, "route degraded");
                Ok(())
            }
            Err(e) => Err(HandlerError::Retry(e.to_string())),
        }
    }
}

/// Room facade for the worker: the module is built with no-op ports (no HTTP, no events); only
/// the repository-backed commands of [`RoomQueries`] are used.
pub fn room_queries(pool: PgPool) -> RoomQueries {
    RoomModule::new(RoomDeps {
        pool,
        jwt: crate::dummy_jwt(),
        clock: Arc::new(SystemClock),
        workspace: Arc::new(NoopWorkspacePolicy),
        consent: Arc::new(NoopVoiceConsentDirectory),
        events: Arc::new(NoopRoomEventPublisher),
    })
    .queries()
}

/// Consumer of `translationRoom:system_events` (group `translationRoom_backend_consumer`).
pub fn route_status_consumer(pool: PgPool) -> (ConsumerConfig, Arc<dyn StreamHandler>) {
    let mut cfg = ConsumerConfig::new(
        TRANSLATION_ROOM_SYSTEM_EVENTS,
        groups::TRANSLATION_ROOM_BACKEND,
    );
    cfg.dead_letter_stream = TRANSLATION_ROOM_SYSTEM_EVENTS_DLQ.to_owned();
    (cfg, Arc::new(RouteDegradedHandler::new(room_queries(pool))))
}
