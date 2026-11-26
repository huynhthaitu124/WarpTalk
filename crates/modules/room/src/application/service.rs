//! Use cases of the translation room service (slice 1).

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Offset, Utc};
use warptalk_contracts::events::AudioRoutesSnapshot;
use warptalk_kernel::{
    AppError, AppResult, Clock, Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId,
};

use super::ports::{
    Caller, ClaimOutcome, Heartbeat, JoinInput, LogInviteNotifier, Mutation, NewBridgeRoom,
    NewCopilotRoom, NewRoom, ParticipantRecord, RoomEventPublisher, RoomInvite, RoomInviteNotifier,
    RoomRecord, RoomRepo, UpdateLanguagesInput, UpdateRoomInput, WorkspacePolicy,
};
use crate::domain::{
    AudioMode, BridgeConfig, BridgeProvider, MAX_INVITES, RoomAction, RoomSettings,
    RoomSettingsPatch, RoomStatus, RoomType, generate_room_code, normalize_invite_emails,
    normalize_meeting_ref, valid_language,
};

/// `now` shifted to the caller's UTC offset; an absent or out-of-range offset (beyond +-14h)
/// falls back to UTC.
fn local_time(now: DateTime<Utc>, utc_offset_minutes: Option<i32>) -> DateTime<FixedOffset> {
    let offset = utc_offset_minutes
        .filter(|m| m.abs() <= 14 * 60)
        .and_then(|m| FixedOffset::east_opt(m * 60))
        .unwrap_or_else(|| Utc.fix());
    now.with_timezone(&offset)
}

pub struct ClaimBridge {
    pub workspace_id: WorkspaceId,
    /// `google_meet | zoom | teams | desktop`; empty (or `desktop`) without a link starts a
    /// desktop session that needs no meeting link.
    pub provider: String,
    pub meeting_url: Option<String>,
    pub code: Option<String>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub source_language: String,
    pub target_languages: Vec<String>,
    /// Caption language of the caller as listener (default: first target language).
    pub listen_language: Option<String>,
    /// Applied only when this claim creates the room (default: no lobby approval).
    pub settings: RoomSettingsPatch,
    /// Added (never removed) when the caller is the room's host.
    pub invited_emails: Option<Vec<String>>,
    /// Caller's UTC offset in minutes (east positive) for the default desktop title.
    pub utc_offset_minutes: Option<i32>,
}

pub struct CreateRoom {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub description: Option<String>,
    pub host_display_name: Option<String>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub source_language: String,
    pub target_languages: Vec<String>,
    pub settings: RoomSettingsPatch,
    pub invited_emails: Option<Vec<String>>,
}

#[derive(Default)]
pub struct UpdateRoom {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub scheduled_at: Option<Option<DateTime<Utc>>>,
    pub source_language: Option<String>,
    pub target_languages: Option<Vec<String>>,
    pub settings: RoomSettingsPatch,
    /// Replaces the pending invitation list.
    pub invited_emails: Option<Vec<String>>,
}

pub struct CreateCopilotRoom {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub host_display_name: String,
    pub host_language: String,
    pub other_language: String,
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
    bridge: BridgeConfig,
    invites: Arc<dyn RoomInviteNotifier>,
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
        bridge: BridgeConfig,
    ) -> Self {
        Self {
            repo,
            workspace,
            events,
            clock,
            bridge,
            invites: Arc::new(LogInviteNotifier),
        }
    }

    /// Replace the outbound invitation notifier (default: log only).
    pub fn with_invite_notifier(mut self, n: Arc<dyn RoomInviteNotifier>) -> Self {
        self.invites = n;
        self
    }

    /// Validated, normalized invite list.
    fn check_invites(raw: Option<&Vec<String>>) -> AppResult<Option<Vec<String>>> {
        let Some(raw) = raw else { return Ok(None) };
        let emails = normalize_invite_emails(raw)
            .map_err(|e| validation("INVALID_EMAIL", &format!("Invalid email: {e}")))?;
        if emails.len() > MAX_INVITES {
            return Err(validation(
                "TOO_MANY_INVITES",
                &format!("At most {MAX_INVITES} invitations per request"),
            ));
        }
        Ok(Some(emails))
    }

    /// Store invitations and notify the newly invited ones (notification failures are logged).
    async fn apply_invites(
        &self,
        room: &RoomRecord,
        inviter: UserId,
        emails: Option<Vec<String>>,
        replace: bool,
    ) -> AppResult<()> {
        let Some(emails) = emails else { return Ok(()) };
        let added = self
            .repo
            .set_invitations(room.id, &emails, replace, self.clock.now())
            .await?;
        if added.is_empty() {
            return Ok(());
        }
        let invite = RoomInvite {
            room_id: room.id,
            workspace_id: room.workspace_id,
            room_title: room.title.clone(),
            room_code: room.code.clone(),
            inviter,
            emails: added,
        };
        if let Err(e) = self.invites.notify(invite).await {
            tracing::warn!(room = %room.id, error = %e, "invitation notification failed");
        }
        Ok(())
    }

    /// Pending invitations, shown only to the room's effective host.
    pub async fn invitations_for(
        &self,
        room: &RoomRecord,
        user: UserId,
    ) -> AppResult<Option<Vec<String>>> {
        if room.effective_host() != user {
            return Ok(None);
        }
        Ok(Some(self.repo.invitations(room.id).await?))
    }

    pub fn bridge_config(&self) -> BridgeConfig {
        self.bridge
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
        let invites = Self::check_invites(req.invited_emails.as_ref())?;
        let title = req.title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 255 {
            return Err(validation(
                "INVALID_TITLE",
                "title must be 1-255 characters",
            ));
        }
        let description = req
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned);
        if description
            .as_ref()
            .is_some_and(|d| d.chars().count() > 5000)
        {
            return Err(validation(
                "INVALID_DESCRIPTION",
                "description must be at most 5000 characters",
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
                description: description.clone(),
                code: generate_room_code(&mut rand::thread_rng()),
                status,
                source_language: req.source_language.clone(),
                target_languages: targets.clone(),
                settings: req.settings.apply(RoomSettings::default()),
                scheduled_at: req.scheduled_at,
                now,
            };
            match self.repo.create(new).await {
                Err(AppError::Conflict { code, .. }) if code == "DUPLICATE" => continue,
                Ok(room) => {
                    self.apply_invites(&room, user, invites.clone(), false)
                        .await?;
                    return Ok(room);
                }
                other => return other,
            }
        }
        Err(AppError::internal("could not allocate a unique room code"))
    }

    /// Create an `IN_PROGRESS` copilot room hosted by `user` (a workspace member). No LiveKit seat,
    /// no pipeline command: only the room row and the host participant.
    pub async fn create_copilot(
        &self,
        user: UserId,
        req: CreateCopilotRoom,
    ) -> AppResult<(RoomRecord, warptalk_kernel::ParticipantId)> {
        let title = req.title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 255 {
            return Err(validation(
                "INVALID_TITLE",
                "title must be 1-255 characters",
            ));
        }
        if !valid_language(&req.host_language) || !valid_language(&req.other_language) {
            return Err(validation("INVALID_LANGUAGE", "invalid language code"));
        }
        if self
            .workspace
            .member_role(req.workspace_id, user)
            .await?
            .is_none()
        {
            return Err(AppError::forbidden("Not a member of this workspace"));
        }
        if !self.workspace.is_active(req.workspace_id).await? {
            return Err(AppError::conflict(
                "WORKSPACE_INACTIVE",
                "The workspace is not available",
            ));
        }
        let now = self.clock.now();
        for _ in 0..8 {
            let new = NewCopilotRoom {
                id: RoomId::new(),
                workspace_id: req.workspace_id,
                host: user,
                host_display_name: req.host_display_name.clone(),
                title: title.clone(),
                code: generate_room_code(&mut rand::thread_rng()),
                source_language: req.host_language.clone(),
                target_language: req.other_language.clone(),
                now,
            };
            match self.repo.create_copilot(new).await {
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
    ) -> AppResult<(RoomRecord, ParticipantRecord)> {
        let (room, _) = self.access(id, user).await?;
        self.join_room(room, user, req).await
    }

    pub async fn join_by_code(
        &self,
        code: &str,
        user: UserId,
        req: JoinRoom,
    ) -> AppResult<(RoomRecord, ParticipantRecord)> {
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
    ) -> AppResult<(RoomRecord, ParticipantRecord)> {
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
        if room.room_type == RoomType::CopilotSession {
            return Err(AppError::conflict(
                "NOT_JOINABLE",
                "A copilot session has no other participants.",
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
        let p = m
            .participant
            .ok_or_else(|| AppError::internal("join returned no participant"))?;
        Ok((m.room, p))
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

    pub async fn update(&self, id: RoomId, user: UserId, req: UpdateRoom) -> AppResult<RoomRecord> {
        let invites = Self::check_invites(req.invited_emails.as_ref())?;
        let title = match req.title {
            Some(t) => {
                let t = t.trim().to_owned();
                if t.is_empty() || t.chars().count() > 255 {
                    return Err(validation(
                        "INVALID_TITLE",
                        "title must be 1-255 characters",
                    ));
                }
                Some(t)
            }
            None => None,
        };
        let description = req
            .description
            .map(|d| d.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty()));
        if description
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|d| d.chars().count() > 5000)
        {
            return Err(validation(
                "INVALID_DESCRIPTION",
                "description must be at most 5000 characters",
            ));
        }
        if req
            .source_language
            .as_deref()
            .is_some_and(|l| !valid_language(l))
        {
            return Err(validation(
                "INVALID_LANGUAGE",
                "sourceLanguage must be a valid language code",
            ));
        }
        let target_languages = match req.target_languages {
            Some(ts) => {
                if ts.is_empty() || ts.len() > 20 || !ts.iter().all(|l| valid_language(l)) {
                    return Err(validation(
                        "INVALID_LANGUAGE",
                        "at least one valid targetLanguage is required",
                    ));
                }
                let mut out: Vec<String> = Vec::new();
                for t in ts {
                    if !out.contains(&t) {
                        out.push(t);
                    }
                }
                Some(out)
            }
            None => None,
        };
        let (_, caller) = self.caller(id, user).await?;
        let m = self
            .repo
            .update_room(UpdateRoomInput {
                room: id,
                caller,
                title,
                description,
                scheduled_at: req.scheduled_at,
                source_language: req.source_language,
                target_languages,
                settings: req.settings,
                now: self.clock.now(),
            })
            .await?;
        self.publish(&m, false).await?;
        self.apply_invites(&m.room, user, invites, true).await?;
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

    /// Own speak/listen language change (`none` speak = listen-only). Validated against the room
    /// languages by the repo; republishes the routes snapshot when the room is IN_PROGRESS.
    pub async fn update_languages(
        &self,
        id: RoomId,
        user: UserId,
        speak_language: Option<String>,
        listen_language: Option<String>,
    ) -> AppResult<ParticipantRecord> {
        if speak_language.is_none() && listen_language.is_none() {
            return Err(validation(
                "INVALID_LANGUAGE",
                "speakLanguage or listenLanguage is required",
            ));
        }
        if !speak_language
            .iter()
            .chain(&listen_language)
            .all(|l| valid_language(l))
        {
            return Err(validation(
                "INVALID_LANGUAGE",
                "speakLanguage and listenLanguage must be valid language codes",
            ));
        }
        let m = self
            .repo
            .update_languages(UpdateLanguagesInput {
                room: id,
                user,
                speak_language,
                listen_language,
                now: self.clock.now(),
            })
            .await?;
        self.publish(&m, false).await?;
        m.participant
            .ok_or_else(|| AppError::internal("update returned no participant"))
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

    // ------------------------------------------------------------ external bridge (D2)

    /// Find-or-create the open bridge room for a meeting; the caller becomes capturer when the
    /// room is new or has none.
    pub async fn claim_bridge(&self, user: UserId, req: ClaimBridge) -> AppResult<ClaimOutcome> {
        let invites = Self::check_invites(req.invited_emails.as_ref())?;
        let raw = req
            .meeting_url
            .as_deref()
            .or(req.code.as_deref())
            .unwrap_or("")
            .trim()
            .to_owned();
        let provider_in = req.provider.trim();
        let desktop = provider_in == "desktop" || (provider_in.is_empty() && raw.is_empty());
        let provider = if desktop {
            BridgeProvider::Desktop
        } else {
            BridgeProvider::parse(provider_in).ok_or_else(|| {
                validation(
                    "INVALID_PROVIDER",
                    "provider must be google_meet, zoom, teams or desktop",
                )
            })?
        };
        // A desktop session has no meeting to share: every start is its own room.
        let meeting_code = if desktop && raw.is_empty() {
            uuid::Uuid::now_v7().simple().to_string()
        } else {
            normalize_meeting_ref(provider, &raw)
                .map_err(|e| validation("INVALID_MEETING_REF", &e.to_string()))?
        };
        let join_url = req
            .meeting_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_owned);
        if join_url.as_ref().is_some_and(|u| u.chars().count() > 1000) {
            return Err(validation("INVALID_MEETING_REF", "meetingUrl is too long"));
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
        if let Some(l) = req.listen_language.as_deref()
            && !valid_language(l)
        {
            return Err(validation(
                "INVALID_LANGUAGE",
                "listenLanguage is not a valid language",
            ));
        }
        let title = match req.title.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => t.to_owned(),
            _ if desktop => format!(
                "Meeting {}",
                local_time(self.clock.now(), req.utc_offset_minutes).format("%d %b %H:%M")
            ),
            _ => format!("External meeting {meeting_code}"),
        };
        if title.chars().count() > 255 {
            return Err(validation(
                "INVALID_TITLE",
                "title must be 1-255 characters",
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
        if !self.workspace.is_active(req.workspace_id).await? {
            return Err(AppError::conflict(
                "WORKSPACE_INACTIVE",
                "The workspace is not available",
            ));
        }
        let mut targets: Vec<String> = Vec::new();
        for t in req.target_languages {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
        for _ in 0..8 {
            let new = NewBridgeRoom {
                id: RoomId::new(),
                workspace_id: req.workspace_id,
                creator: user,
                creator_display_name: req
                    .display_name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "Host".into()),
                title: title.clone(),
                code: generate_room_code(&mut rand::thread_rng()),
                provider,
                meeting_code: meeting_code.clone(),
                join_url: join_url.clone(),
                source_language: req.source_language.clone(),
                target_languages: targets.clone(),
                display_name: req.display_name.clone(),
                listen_language: req.listen_language.clone(),
                settings: req.settings.apply(RoomSettings {
                    requires_approval: false,
                    ..RoomSettings::default()
                }),
                now: self.clock.now(),
            };
            match self.repo.claim_bridge(new, self.bridge).await {
                Err(AppError::Conflict { code, .. }) if code == "DUPLICATE" => continue,
                Ok(mut out) => {
                    if let Some(s) = out.snapshot.take()
                        && let Err(e) = self.events.publish_routes(out.room.id, &s).await
                    {
                        tracing::warn!(room = %out.room.id, error = %e, "route publication failed");
                    }
                    if out.room.effective_host() == user {
                        self.apply_invites(&out.room, user, invites.clone(), false)
                            .await?;
                    }
                    return Ok(out);
                }
                other => return other,
            }
        }
        Err(AppError::internal("could not allocate a unique room code"))
    }

    /// Capturer heartbeat (every `heartbeat_interval`).
    pub async fn bridge_heartbeat(
        &self,
        id: RoomId,
        user: UserId,
        audio_flowing: bool,
        last_audio_at: Option<DateTime<Utc>>,
    ) -> AppResult<RoomRecord> {
        self.access(id, user).await?;
        self.repo
            .bridge_heartbeat(
                id,
                user,
                Heartbeat {
                    audio_flowing,
                    last_audio_at,
                },
                self.clock.now(),
            )
            .await
    }

    pub async fn bridge_takeover(&self, id: RoomId, user: UserId) -> AppResult<RoomRecord> {
        let (_, caller) = self.access(id, user).await?;
        self.repo
            .bridge_takeover(id, caller, self.bridge, self.clock.now())
            .await
    }

    /// Capturer or host gives up the capturer seat immediately.
    pub async fn bridge_release(&self, id: RoomId, user: UserId) -> AppResult<RoomRecord> {
        let (_, caller) = self.access(id, user).await?;
        self.repo.bridge_release(id, caller, self.clock.now()).await
    }

    /// Server clock, for `serverTime` in responses.
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub async fn set_bridge_audio_mode(
        &self,
        id: RoomId,
        user: UserId,
        mode: AudioMode,
    ) -> AppResult<RoomRecord> {
        let (_, caller) = self.access(id, user).await?;
        self.repo
            .set_bridge_audio_mode(id, caller, mode, self.clock.now())
            .await
    }
}

#[cfg(test)]
mod local_time_tests {
    use super::local_time;
    use chrono::{TimeZone, Utc};

    #[test]
    fn shifts_to_caller_offset_and_falls_back_to_utc() {
        let now = Utc.with_ymd_and_hms(2026, 11, 26, 11, 30, 0).single();
        let Some(now) = now else { return };
        assert_eq!(
            local_time(now, Some(420)).format("%H:%M").to_string(),
            "18:30"
        );
        assert_eq!(
            local_time(now, Some(-300)).format("%d %H:%M").to_string(),
            "26 06:30"
        );
        assert_eq!(local_time(now, None).format("%H:%M").to_string(), "11:30");
        assert_eq!(
            local_time(now, Some(24 * 60)).format("%H:%M").to_string(),
            "11:30"
        );
    }
}
