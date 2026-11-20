//! Use cases of the translation room service (slice 1).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use warptalk_contracts::events::AudioRoutesSnapshot;
use warptalk_kernel::{
    AppError, AppResult, Clock, Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId,
};

use super::ports::{
    Caller, JoinInput, Mutation, NewRoom, ParticipantRecord, RoomEventPublisher, RoomRecord,
    RoomRepo, WorkspacePolicy,
};
use crate::domain::{RoomAction, RoomStatus, generate_room_code, valid_language};

pub struct CreateRoom {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub host_display_name: Option<String>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub requires_approval: Option<bool>,
}

pub struct JoinRoom {
    pub display_name: Option<String>,
    pub speak_language: String,
    pub listen_language: String,
}

pub struct RoomService {
    repo: Arc<dyn RoomRepo>,
    workspace: Arc<dyn WorkspacePolicy>,
    events: Arc<dyn RoomEventPublisher>,
    clock: Arc<dyn Clock>,
}

fn not_found() -> AppError {
    AppError::not_found("TranslationRoom not found")
}

fn validation(code: &str, msg: &str) -> AppError {
    AppError::validation(code, msg)
}

impl RoomService {
    pub fn new(
        repo: Arc<dyn RoomRepo>,
        workspace: Arc<dyn WorkspacePolicy>,
        events: Arc<dyn RoomEventPublisher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            workspace,
            events,
            clock,
        }
    }

    /// Room plus caller context. Outsiders (neither workspace member nor participant) get NotFound.
    async fn access(&self, id: RoomId, user: UserId) -> AppResult<(RoomRecord, Caller)> {
        let room = self.repo.get(id).await?.ok_or_else(not_found)?;
        let ws_role = self.workspace.member_role(room.workspace_id, user).await?;
        if ws_role.is_none() && !self.repo.has_participant(id, user).await? {
            return Err(not_found());
        }
        Ok((room, Caller { user, ws_role }))
    }

    /// Like [`access`] but never leaks existence: any user gets a caller context (for host-only
    /// actions the repo answers Forbidden/NotFound under the lock).
    async fn caller(&self, id: RoomId, user: UserId) -> AppResult<(RoomRecord, Caller)> {
        let room = self.repo.get(id).await?.ok_or_else(not_found)?;
        let ws_role = self.workspace.member_role(room.workspace_id, user).await?;
        Ok((room, Caller { user, ws_role }))
    }

    async fn publish(&self, m: &Mutation, routes_required: bool) -> AppResult<()> {
        if let Some(s) = &m.snapshot
            && let Err(e) = self.events.publish_routes(m.room.id, s).await
        {
            if routes_required {
                tracing::error!(room = %m.room.id, error = %e, "route publication failed");
                return Err(AppError::unavailable("audio routes could not be published"));
            }
            tracing::warn!(room = %m.room.id, error = %e, "route publication failed");
        }
        for c in &m.commands {
            if let Err(e) = self.events.publish_command(c).await {
                tracing::warn!(room = %m.room.id, error = %e, "room command publication failed");
            }
        }
        Ok(())
    }

    pub async fn create(&self, user: UserId, req: CreateRoom) -> AppResult<RoomRecord> {
        let title = req.title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 255 {
            return Err(validation(
                "INVALID_TITLE",
                "title must be 1-255 characters",
            ));
        }
        if !valid_language(&req.source_language)
            || req.target_languages.is_empty()
            || req.target_languages.len() > 20
            || !req.target_languages.iter().all(|l| valid_language(l))
        {
            return Err(validation(
                "INVALID_LANGUAGE",
                "sourceLanguage and at least one valid targetLanguage are required",
            ));
        }
        if self
            .workspace
            .member_role(req.workspace_id, user)
            .await?
            .is_none()
        {
            return Err(AppError::forbidden("Not a member of this workspace"));
        }
        let now = self.clock.now();
        let status = match req.scheduled_at {
            Some(t) if t > now => RoomStatus::Scheduled,
            _ => RoomStatus::Waiting,
        };
        let mut targets: Vec<String> = Vec::new();
        for t in req.target_languages {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
        for _ in 0..8 {
            let new = NewRoom {
                id: RoomId::new(),
                workspace_id: req.workspace_id,
                host: user,
                host_display_name: req
                    .host_display_name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "Host".into()),
                title: title.clone(),
                code: generate_room_code(&mut rand::thread_rng()),
                status,
                source_language: req.source_language.clone(),
                target_languages: targets.clone(),
                requires_approval: req.requires_approval.unwrap_or(true),
                scheduled_at: req.scheduled_at,
                now,
            };
            match self.repo.create(new).await {
                Err(AppError::Conflict { code, .. }) if code == "DUPLICATE" => continue,
                other => return other,
            }
        }
        Err(AppError::internal("could not allocate a unique room code"))
    }

    pub async fn list(
        &self,
        user: UserId,
        workspace: WorkspaceId,
        status: Option<RoomStatus>,
        page: &PageRequest,
    ) -> AppResult<Page<RoomRecord>> {
        if self.workspace.member_role(workspace, user).await?.is_none() {
            return Err(AppError::forbidden("Not a member of this workspace"));
        }
        self.repo.list(workspace, status, page).await
    }

    pub async fn get(&self, id: RoomId, user: UserId) -> AppResult<RoomRecord> {
        Ok(self.access(id, user).await?.0)
    }

    pub async fn join_by_id(
        &self,
        id: RoomId,
        user: UserId,
        req: JoinRoom,
    ) -> AppResult<ParticipantRecord> {
        let (room, _) = self.access(id, user).await?;
        self.join_room(room, user, req).await
    }

    pub async fn join_by_code(
        &self,
        code: &str,
        user: UserId,
        req: JoinRoom,
    ) -> AppResult<ParticipantRecord> {
        let room = self
            .repo
            .find_by_code(code.trim().to_lowercase().as_str())
            .await?
            .ok_or_else(not_found)?;
        self.join_room(room, user, req).await
    }

    async fn join_room(
        &self,
        room: RoomRecord,
        user: UserId,
        req: JoinRoom,
    ) -> AppResult<ParticipantRecord> {
        if !valid_language(&req.speak_language) || !valid_language(&req.listen_language) {
            return Err(validation(
                "INVALID_LANGUAGE",
                "speakLanguage and listenLanguage are required",
            ));
        }
        if let Some(n) = &req.display_name
            && (n.trim().is_empty() || n.chars().count() > 100)
        {
            return Err(validation(
                "INVALID_DISPLAY_NAME",
                "displayName must be 1-100 characters",
            ));
        }
        if room.status.is_terminal() {
            return Err(AppError::conflict(
                "INVALID_STATE",
                "This room has already ended or has been cancelled.",
            ));
        }
        if !self.workspace.is_active(room.workspace_id).await? {
            return Err(AppError::conflict(
                "WORKSPACE_INACTIVE",
                "The workspace is not available",
            ));
        }
        let m = self
            .repo
            .join(JoinInput {
                room: room.id,
                user,
                display_name: req.display_name.map(|n| n.trim().to_owned()),
                speak_language: req.speak_language,
                listen_language: req.listen_language,
                now: self.clock.now(),
            })
            .await?;
        self.publish(&m, false).await?;
        m.participant
            .ok_or_else(|| AppError::internal("join returned no participant"))
    }

    pub async fn lifecycle(
        &self,
        id: RoomId,
        user: UserId,
        action: RoomAction,
    ) -> AppResult<RoomRecord> {
        let (room, caller) = self.caller(id, user).await?;
        if action == RoomAction::Start && !self.workspace.is_active(room.workspace_id).await? {
            return Err(AppError::conflict(
                "WORKSPACE_INACTIVE",
                "The workspace is not available",
            ));
        }
        let m = self
            .repo
            .lifecycle(id, caller, action, self.clock.now())
            .await?;
        self.publish(&m, action == RoomAction::Start).await?;
        Ok(m.room)
    }

    pub async fn participants(
        &self,
        id: RoomId,
        user: UserId,
    ) -> AppResult<Vec<ParticipantRecord>> {
        self.access(id, user).await?;
        self.repo.participants(id).await
    }

    pub async fn admit(
        &self,
        id: RoomId,
        participant: ParticipantId,
        user: UserId,
    ) -> AppResult<ParticipantRecord> {
        let (room, caller) = self.caller(id, user).await?;
        if !self.workspace.is_active(room.workspace_id).await? {
            return Err(AppError::conflict(
                "WORKSPACE_INACTIVE",
                "The workspace is not available",
            ));
        }
        let m = self
            .repo
            .admit(id, participant, caller, self.clock.now())
            .await?;
        self.publish(&m, false).await?;
        m.participant
            .ok_or_else(|| AppError::internal("admit returned no participant"))
    }

    pub async fn kick(
        &self,
        id: RoomId,
        participant: ParticipantId,
        user: UserId,
    ) -> AppResult<ParticipantRecord> {
        let (_, caller) = self.caller(id, user).await?;
        let m = self
            .repo
            .kick(id, participant, caller, self.clock.now())
            .await?;
        self.publish(&m, false).await?;
        m.participant
            .ok_or_else(|| AppError::internal("kick returned no participant"))
    }

    pub async fn leave(&self, id: RoomId, user: UserId) -> AppResult<ParticipantRecord> {
        let m = self.repo.leave(id, user, self.clock.now()).await?;
        self.publish(&m, false).await?;
        m.participant
            .ok_or_else(|| AppError::internal("leave returned no participant"))
    }

    pub async fn generate_routes(
        &self,
        id: RoomId,
        user: UserId,
    ) -> AppResult<AudioRoutesSnapshot> {
        let (_, caller) = self.caller(id, user).await?;
        let m = self
            .repo
            .generate_routes(id, caller, self.clock.now())
            .await?;
        self.publish(&m, false).await?;
        m.snapshot
            .ok_or_else(|| AppError::internal("generate returned no snapshot"))
    }

    pub async fn routes(&self, id: RoomId, user: UserId) -> AppResult<AudioRoutesSnapshot> {
        self.access(id, user).await?;
        self.repo.routes_snapshot(id, self.clock.now()).await
    }
}
