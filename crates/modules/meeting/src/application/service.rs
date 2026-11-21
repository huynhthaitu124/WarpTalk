//! Meeting use cases: join, bridge token, end, LiveKit webhook handling.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, UserId};
use warptalk_livekit::{
    AccessTokenBuilder, LiveKitConfig, LiveKitError, RoomService, WebhookEvent, participant_token,
    verify_webhook,
};

use super::ports::{
    EVENT_BILLING_STOP, EVENT_END_ROOM, MeetingEvent, MeetingEventPublisher, MeetingRepo,
    MeetingRoom, NewParticipant, RoomDirectory, RoomInfo,
};
use crate::domain::{
    BRIDGE_IDENTITY, JoinGate, STATUS_FINISHED, WebhookKind, is_bridge_audio_owner,
    is_ended_status, join_gate,
};

/// LiveKit room empty timeout (seconds) used on CreateRoom.
const EMPTY_TIMEOUT_SECS: u32 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGrant {
    pub room_name: String,
    pub server_url: String,
    pub token: String,
    pub participant_identity: String,
}

pub struct MeetingService {
    repo: Arc<dyn MeetingRepo>,
    rooms: Arc<dyn RoomDirectory>,
    room_service: Arc<dyn RoomService>,
    events: Arc<dyn MeetingEventPublisher>,
    clock: Arc<dyn Clock>,
    livekit: LiveKitConfig,
    token_ttl: Duration,
}

fn lk_err(e: LiveKitError) -> AppError {
    tracing::error!(error = %e, "livekit call failed");
    AppError::unavailable("media server unavailable")
}

impl MeetingService {
    pub fn new(
        repo: Arc<dyn MeetingRepo>,
        rooms: Arc<dyn RoomDirectory>,
        room_service: Arc<dyn RoomService>,
        events: Arc<dyn MeetingEventPublisher>,
        clock: Arc<dyn Clock>,
        livekit: LiveKitConfig,
        token_ttl: Duration,
    ) -> Self {
        Self {
            repo,
            rooms,
            room_service,
            events,
            clock,
            livekit,
            token_ttl,
        }
    }

    async fn room_info(&self, room: RoomId) -> AppResult<RoomInfo> {
        self.rooms
            .room(room)
            .await?
            .ok_or_else(|| AppError::not_found("translation room not found"))
    }

    fn is_host(info: &RoomInfo, meeting: Option<&MeetingRoom>, user: UserId) -> bool {
        info.host_id == user || meeting.is_some_and(|m| m.active_host_id == Some(user))
    }

    /// Spec: 11 §5.1 (gate, room provisioning, participant upsert, token).
    pub async fn join(
        &self,
        user: UserId,
        room: RoomId,
        display_name: Option<String>,
    ) -> AppResult<JoinGrant> {
        let info = self.room_info(room).await?;
        if is_ended_status(&info.status) {
            return Err(AppError::conflict(
                "INVALID_STATE",
                "meeting has already ended",
            ));
        }
        let existing = self.repo.find_current(room).await?;
        let is_host = Self::is_host(&info, existing.as_ref(), user);
        let participant = self.rooms.participant(room, user).await?;
        match join_gate(is_host, participant.as_ref().map(|p| p.status.as_str())) {
            JoinGate::Allow => {}
            JoinGate::Lobby => {
                return Err(AppError::coded(
                    403,
                    "IN_LOBBY",
                    "waiting for the host to admit you",
                ));
            }
            JoinGate::Denied => {
                return Err(AppError::forbidden("not a participant of this meeting"));
            }
        }

        let now = self.clock.now();
        let room_name = room.to_string();
        let meeting = match existing {
            Some(m) if m.status == STATUS_FINISHED => {
                return Err(AppError::conflict(
                    "INVALID_STATE",
                    "meeting has already ended",
                ));
            }
            Some(m) => {
                self.repo.activate(m.id, now).await?;
                m
            }
            None => {
                // CreateRoom is idempotent on the LiveKit side; do it before the row exists so a
                // failure leaves nothing behind and the next join retries.
                self.room_service
                    .create_room(&room_name, EMPTY_TIMEOUT_SECS, "")
                    .await
                    .map_err(lk_err)?;
                self.repo
                    .insert_room(room, &room_name, info.host_id, now)
                    .await?;
                self.repo
                    .find_current(room)
                    .await?
                    .ok_or_else(|| AppError::internal("meeting room vanished after insert"))?
            }
        };
        if is_host && meeting.active_host_id.is_none() {
            self.repo.claim_host(meeting.id, user, now).await?;
        }

        let identity = user.to_string();
        let name = display_name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .or_else(|| participant.map(|p| p.display_name))
            .unwrap_or_else(|| identity.clone());
        self.repo
            .upsert_participant(NewParticipant {
                meeting_room_id: meeting.id,
                user_id: user,
                identity: &identity,
                display_name: &name,
                now,
            })
            .await?;

        let token = participant_token(
            &self.livekit,
            &room_name,
            &identity,
            &name,
            self.token_ttl,
            None,
        )
        .map_err(lk_err)?;
        Ok(JoinGrant {
            room_name,
            server_url: self.livekit.url.clone(),
            token,
            participant_identity: identity,
        })
    }

    /// Load the live session and require `user` to be its host.
    async fn host_session(&self, user: UserId, room: RoomId) -> AppResult<MeetingRoom> {
        let info = self.room_info(room).await?;
        let meeting = self
            .repo
            .find_current(room)
            .await?
            .ok_or_else(|| AppError::not_found("meeting has not started"))?;
        if !Self::is_host(&info, Some(&meeting), user) {
            return Err(AppError::forbidden("only the host can do this"));
        }
        Ok(meeting)
    }

    /// Spec: 11 §3.1 `bridge-token` (publish-only seat). Room must be EXTERNAL_BRIDGE (409
    /// `NOT_BRIDGE_ROOM`); caller must be the current non-stale capturer, or the host while the
    /// room has no capturer (403 `NOT_CAPTURER`).
    pub async fn bridge_token(&self, user: UserId, room: RoomId) -> AppResult<JoinGrant> {
        let info = self.room_info(room).await?;
        if is_ended_status(&info.status) {
            return Err(AppError::conflict(
                "INVALID_STATE",
                "meeting has already ended",
            ));
        }
        let bridge = self.rooms.bridge(room).await?.ok_or_else(|| {
            AppError::conflict(
                "NOT_BRIDGE_ROOM",
                "this room is not an external bridge room",
            )
        })?;
        if !is_bridge_audio_owner(
            bridge.capturer_user_id.map(|u| u.as_uuid()),
            bridge.capturer_stale,
            info.host_id.as_uuid(),
            user.as_uuid(),
        ) {
            return Err(AppError::coded(
                403,
                "NOT_CAPTURER",
                "only the current capturer can publish the external meeting audio",
            ));
        }
        let meeting = self
            .repo
            .find_current(room)
            .await?
            .ok_or_else(|| AppError::not_found("meeting has not started"))?;
        if meeting.status == STATUS_FINISHED {
            return Err(AppError::conflict(
                "INVALID_STATE",
                "meeting has already ended",
            ));
        }
        let metadata =
            json!({ "provider": bridge.provider, "role": "external_bridge" }).to_string();
        let token = AccessTokenBuilder::new(&self.livekit, BRIDGE_IDENTITY)
            .name("External Meeting")
            .metadata(metadata)
            .ttl(self.token_ttl)
            .room(meeting.provider_room_name.clone())
            .room_join(true)
            .can_publish(true)
            .can_subscribe(false)
            .can_publish_data(false)
            .can_publish_sources(vec!["MICROPHONE".to_owned()])
            .build()
            .map_err(lk_err)?;
        Ok(JoinGrant {
            room_name: meeting.provider_room_name,
            server_url: self.livekit.url.clone(),
            token,
            participant_identity: BRIDGE_IDENTITY.to_owned(),
        })
    }

    /// Spec: 11 §4.3 (host check, DeleteRoom first, single winner marks FINISHED and publishes).
    pub async fn end(&self, user: UserId, room: RoomId) -> AppResult<()> {
        let meeting = self.host_session(user, room).await?;
        let info = self.room_info(room).await?;
        match self
            .room_service
            .delete_room(&meeting.provider_room_name)
            .await
        {
            Ok(()) => {}
            Err(e) if e.is_not_found() => {}
            Err(e) => return Err(lk_err(e)),
        }
        let now = self.clock.now();
        if !self.repo.try_mark_finished(meeting.id, now).await? {
            return Ok(()); // already finished by another call: idempotent
        }
        let base = json!({
            "translation_room_id": room,
            "workspace_id": info.workspace_id,
            "room_name": meeting.provider_room_name,
            "ended_at": now,
        });
        for event_type in [EVENT_END_ROOM, EVENT_BILLING_STOP] {
            let ev = MeetingEvent {
                event_type,
                payload: base.clone(),
            };
            if let Err(e) = self.events.publish(ev).await {
                // Let the host retry: reopen so the next End wins again (DeleteRoom is idempotent).
                if let Err(re) = self.repo.reopen(meeting.id, self.clock.now()).await {
                    tracing::error!(error = %re, "failed to reopen meeting after publish failure");
                }
                return Err(e);
            }
        }
        Ok(())
    }

    /// Verify and apply a LiveKit webhook. Safe under redelivery and reordering.
    pub async fn handle_webhook(&self, body: &[u8], auth_header: &str) -> AppResult<()> {
        let ev = verify_webhook(body, auth_header, &self.livekit).map_err(|e| {
            if e.is_auth_failure() {
                tracing::warn!(error = %e, "rejected livekit webhook");
                AppError::unauthorized("invalid webhook signature")
            } else {
                AppError::validation("INVALID_WEBHOOK", "malformed webhook body")
            }
        })?;
        self.apply_event(&ev).await
    }

    async fn apply_event(&self, ev: &WebhookEvent) -> AppResult<()> {
        let kind = WebhookKind::parse(&ev.event);
        if kind == WebhookKind::Ignored {
            return Ok(());
        }
        let Some(room_name) = ev
            .room
            .as_ref()
            .map(|r| r.name.as_str())
            .filter(|n| !n.is_empty())
        else {
            return Ok(());
        };
        let Some(meeting) = self.repo.find_by_provider_name(room_name).await? else {
            tracing::debug!(room = room_name, "webhook for unknown room ignored");
            return Ok(());
        };
        let at: DateTime<Utc> = ev
            .created_at
            .and_then(|s| DateTime::from_timestamp(s, 0))
            .unwrap_or_else(|| self.clock.now());
        let identity = ev
            .participant
            .as_ref()
            .map(|p| p.identity.as_str())
            .filter(|i| !i.is_empty());

        match (kind, identity) {
            (WebhookKind::ParticipantJoined, Some(identity)) => {
                self.repo
                    .participant_joined(meeting.id, identity, at)
                    .await?;
            }
            (WebhookKind::ParticipantLeft, Some(identity)) => {
                if self.repo.participant_left(meeting.id, identity, at).await? {
                    self.repo
                        .clear_active_host_if(meeting.id, identity, self.clock.now())
                        .await?;
                }
            }
            (WebhookKind::Track(change), Some(identity)) => {
                if let Some(track) = ev.track.as_ref().filter(|t| !t.sid.is_empty()) {
                    self.repo
                        .apply_track(
                            meeting.id,
                            identity,
                            &track.sid,
                            track.media_type(),
                            change,
                            at,
                        )
                        .await?;
                }
            }
            (WebhookKind::RoomFinished, _) => {
                self.repo.close_room(meeting.id, at).await?;
            }
            _ => {}
        }
        Ok(())
    }
}
