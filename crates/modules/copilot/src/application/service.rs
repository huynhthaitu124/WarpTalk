//! Use cases: session lifecycle, segment ingest, ephemeral STT token.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{
    AppError, AppResult, Clock, Page, PageRequest, RoomId, UserId, WorkspaceId, WorkspaceRole,
};

use super::ports::{
    Metering, NewSessionRoom, Published, RateLimiter, RoomState, SegmentSink, SessionRecord,
    SessionRepo, SessionRooms, SttIssueError, SttToken, SttTokenIssuer, SttTokenRequest,
    UsageEvent, WorkspaceMembers,
};
use crate::domain::{
    DEFAULT_NAME_ME, DEFAULT_NAME_OTHERS, DEFAULT_TITLE, MAX_SEGMENTS_PER_CALL, Platform, Segment,
    SessionFacts, others_speaker_uuid, segment_uuid, to_stt_result, valid_language,
};

/// `stt-token` calls per user per window.
pub const STT_TOKEN_LIMIT: u32 = 10;
pub const STT_TOKEN_WINDOW_SECS: u64 = 60;

pub struct CreateSession {
    pub workspace_id: WorkspaceId,
    pub title: Option<String>,
    /// `google_meet | zoom | teams | other | in_person`.
    pub platform: String,
    pub meeting_url: Option<String>,
    pub language_me: String,
    pub language_others: String,
    pub name_me: Option<String>,
    pub name_others: Option<String>,
}

/// A session with its room state.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub record: SessionRecord,
    /// Room status (`IN_PROGRESS`, `ENDED`, ...).
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// Result of one ingest call.
#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub accepted: usize,
    pub duplicates: usize,
    /// Per input segment, in order: `(clientSegmentId, segmentId, duplicate)`.
    pub segments: Vec<(String, Uuid, bool)>,
    /// Billing suspended AI usage for the workspace (no credits left). The segments were still
    /// accepted (the transcript is never dropped); the app should tell the user.
    pub insufficient_credits: bool,
}

pub struct CopilotService {
    repo: Arc<dyn SessionRepo>,
    rooms: Arc<dyn SessionRooms>,
    members: Arc<dyn WorkspaceMembers>,
    sink: Arc<dyn SegmentSink>,
    issuer: Arc<dyn SttTokenIssuer>,
    limiter: Arc<dyn RateLimiter>,
    clock: Arc<dyn Clock>,
    metering: Option<Metering>,
}

fn not_found() -> AppError {
    AppError::not_found("Copilot session not found")
}

fn is_admin(role: Option<WorkspaceRole>) -> bool {
    matches!(role, Some(WorkspaceRole::Owner | WorkspaceRole::Admin))
}

fn clean_name(raw: Option<String>, default: &str) -> AppResult<String> {
    match raw.map(|n| n.trim().to_owned()) {
        None => Ok(default.to_owned()),
        Some(n) if n.is_empty() => Ok(default.to_owned()),
        Some(n) if n.chars().count() > 100 => Err(AppError::validation(
            "INVALID_DISPLAY_NAME",
            "display names must be at most 100 characters",
        )),
        Some(n) => Ok(n),
    }
}

impl CopilotService {
    pub fn new(
        repo: Arc<dyn SessionRepo>,
        rooms: Arc<dyn SessionRooms>,
        members: Arc<dyn WorkspaceMembers>,
        sink: Arc<dyn SegmentSink>,
        issuer: Arc<dyn SttTokenIssuer>,
        limiter: Arc<dyn RateLimiter>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            rooms,
            members,
            sink,
            issuer,
            limiter,
            clock,
            metering: None,
        }
    }

    /// Enable usage events and the credit gate (billing).
    pub fn set_metering(&mut self, m: Metering) {
        self.metering = Some(m);
    }

    fn view(record: SessionRecord, room: RoomState) -> SessionView {
        let started_at = room.started_at.unwrap_or(record.created_at);
        SessionView {
            record,
            status: room.status,
            started_at,
            ended_at: room.ended_at,
        }
    }

    pub async fn create(&self, user: UserId, req: CreateSession) -> AppResult<SessionView> {
        let platform = Platform::parse(&req.platform).ok_or_else(|| {
            AppError::validation(
                "INVALID_PLATFORM",
                "platform must be google_meet, zoom, teams, other or in_person",
            )
        })?;
        if !valid_language(&req.language_me) || !valid_language(&req.language_others) {
            return Err(AppError::validation(
                "INVALID_LANGUAGE",
                "languages.me and languages.others must be language tags such as `en` or `vi`",
            ));
        }
        let meeting_url = req
            .meeting_url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty());
        if let Some(u) = &meeting_url
            && (u.chars().count() > 1000
                || !(u.starts_with("https://") || u.starts_with("http://")))
        {
            return Err(AppError::validation(
                "INVALID_MEETING_URL",
                "meetingUrl must be an http(s) URL of at most 1000 characters",
            ));
        }
        let title = req
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| DEFAULT_TITLE.to_owned());
        if title.chars().count() > 255 {
            return Err(AppError::validation(
                "INVALID_TITLE",
                "title must be at most 255 characters",
            ));
        }
        let name_me = clean_name(req.name_me, DEFAULT_NAME_ME)?;
        let name_others = clean_name(req.name_others, DEFAULT_NAME_OTHERS)?;

        let created = self
            .rooms
            .create(
                user,
                NewSessionRoom {
                    workspace_id: req.workspace_id,
                    title: title.clone(),
                    host_display_name: name_me.clone(),
                    host_language: req.language_me.clone(),
                    other_language: req.language_others.clone(),
                },
            )
            .await?;
        let record = SessionRecord {
            room_id: created.room_id,
            workspace_id: req.workspace_id,
            host_id: user,
            host_participant_id: created.host_participant_id,
            others_speaker_id: others_speaker_uuid(created.room_id),
            title,
            platform,
            meeting_url,
            language_me: req.language_me,
            language_others: req.language_others,
            name_me,
            name_others,
            created_at: created.started_at,
        };
        if let Err(e) = self.repo.insert(&record).await {
            // Do not leave an orphan room running.
            if let Err(e2) = self.rooms.end(created.room_id, user).await {
                tracing::warn!(room = %created.room_id, error = %e2, "could not end the orphan room");
            }
            return Err(e);
        }
        Ok(SessionView {
            status: "IN_PROGRESS".into(),
            started_at: created.started_at,
            ended_at: None,
            record,
        })
    }

    /// Host, or workspace owner/admin. Everybody else (and non-members) gets 404.
    async fn load_viewable(&self, id: RoomId, user: UserId) -> AppResult<SessionView> {
        let record = self.repo.get(id).await?.ok_or_else(not_found)?;
        if record.host_id != user {
            let role = self.members.member_role(record.workspace_id, user).await?;
            if !is_admin(role) {
                return Err(not_found());
            }
        }
        let room = self.rooms.state(id).await?.ok_or_else(not_found)?;
        Ok(Self::view(record, room))
    }

    pub async fn get(&self, id: RoomId, user: UserId) -> AppResult<SessionView> {
        self.load_viewable(id, user).await
    }

    /// Members see their own sessions; owners/admins see the whole workspace.
    pub async fn list(
        &self,
        user: UserId,
        ws: WorkspaceId,
        page: &PageRequest,
    ) -> AppResult<Page<SessionView>> {
        let role = self
            .members
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::forbidden("Not a member of this workspace"))?;
        let host = (!is_admin(Some(role))).then_some(user);
        let (records, total) = self
            .repo
            .list(ws, host, page.limit(), page.offset())
            .await?;
        let mut items = Vec::with_capacity(records.len());
        for r in records {
            // A room that vanished (hard delete) is skipped rather than failing the list.
            if let Some(room) = self.rooms.state(r.room_id).await? {
                items.push(Self::view(r, room));
            }
        }
        Ok(Page::new(items, page, total))
    }

    /// End the session (host or workspace owner/admin; idempotent). The room end path publishes
    /// `__MEETING_END__` so summaries and suggestion cleanup run.
    pub async fn end(&self, id: RoomId, user: UserId) -> AppResult<SessionView> {
        let v = self.load_viewable(id, user).await?;
        if v.status == "ENDED" {
            return Ok(v);
        }
        self.rooms.end(id, user).await?;
        self.load_viewable(id, user).await
    }

    /// Host-only access to a running session.
    async fn load_host_running(
        &self,
        id: RoomId,
        user: UserId,
    ) -> AppResult<(SessionRecord, RoomState)> {
        let record = self.repo.get(id).await?.ok_or_else(not_found)?;
        if record.host_id != user {
            // Outsiders must not learn the session exists.
            let role = self.members.member_role(record.workspace_id, user).await?;
            return Err(if role.is_some() {
                AppError::forbidden("Only the host can do this")
            } else {
                not_found()
            });
        }
        let room = self.rooms.state(id).await?.ok_or_else(not_found)?;
        if !room.in_progress() {
            return Err(AppError::conflict(
                "SESSION_NOT_IN_PROGRESS",
                "The session is not in progress",
            ));
        }
        Ok((record, room))
    }

    /// Validate every segment, then write each to `stt:results` (and the per-meeting copy).
    /// Resending a `clientSegmentId` is a no-op reported as a duplicate.
    pub async fn ingest(
        &self,
        id: RoomId,
        user: UserId,
        segments: Vec<Segment>,
    ) -> AppResult<IngestOutcome> {
        if segments.is_empty() || segments.len() > MAX_SEGMENTS_PER_CALL {
            return Err(AppError::validation(
                "INVALID_SEGMENTS",
                format!("send between 1 and {MAX_SEGMENTS_PER_CALL} segments per call"),
            ));
        }
        for (i, s) in segments.iter().enumerate() {
            s.validate(i)
                .map_err(|e| AppError::validation("INVALID_SEGMENT", e.to_string()))?;
        }
        let (record, room) = self.load_host_running(id, user).await?;
        let facts = SessionFacts {
            room_id: id,
            host_participant_id: record.host_participant_id,
            others_speaker_id: record.others_speaker_id,
            name_me: record.name_me,
            name_others: record.name_others,
            started_at: room.started_at.unwrap_or(record.created_at),
        };
        let now = self.clock.now();
        let mut out = IngestOutcome {
            accepted: 0,
            duplicates: 0,
            segments: Vec::with_capacity(segments.len()),
            insufficient_credits: false,
        };
        for seg in &segments {
            let result = to_stt_result(&facts, seg, now, 0);
            let dedup = format!("{id}:{}", seg.client_segment_id);
            let duplicate = match self.sink.publish(&result, &dedup).await? {
                Published::Written => {
                    out.accepted += 1;
                    self.emit_stt_usage(&record.workspace_id, id, user, seg, now)
                        .await;
                    false
                }
                Published::Duplicate => {
                    out.duplicates += 1;
                    true
                }
            };
            out.segments.push((
                seg.client_segment_id.clone(),
                segment_uuid(id, &seg.client_segment_id),
                duplicate,
            ));
        }
        if let Some(m) = &self.metering {
            out.insufficient_credits = m.gate.suspended(record.workspace_id).await.is_some();
        }
        Ok(out)
    }

    /// One `usage:events` entry per newly accepted segment: audio = the segment's duration.
    /// Never fails the ingest (the transcript matters more than the meter; failures are logged).
    async fn emit_stt_usage(
        &self,
        ws: &WorkspaceId,
        room: RoomId,
        user: UserId,
        seg: &Segment,
        now: DateTime<Utc>,
    ) {
        let Some(m) = &self.metering else { return };
        let audio_ms = (seg.end_ms - seg.start_ms).max(0);
        if audio_ms == 0 {
            return;
        }
        let segment_id = segment_uuid(room, &seg.client_segment_id);
        let event = UsageEvent {
            event_id: format!("stt:{segment_id}"),
            workspace_id: ws.as_uuid(),
            room_id: Some(room.as_uuid()),
            user_id: Some(user.as_uuid()),
            feature: "stt",
            model: m.stt_model.clone(),
            audio_ms: Some(audio_ms),
            boundary: "platform",
            occurred_at: now,
            segment_id: Some(segment_id),
        };
        if let Err(e) = m.usage.emit(&event).await {
            tracing::warn!(session = %room, error = %e, "could not publish usage event");
        }
    }

    /// Mint a short-lived provider token for the host's app (rate limited per user).
    pub async fn stt_token(&self, id: RoomId, user: UserId) -> AppResult<SttToken> {
        let (record, _) = self.load_host_running(id, user).await?;
        if let Some(m) = &self.metering
            && let Some(reason) = m.gate.suspended(record.workspace_id).await
        {
            tracing::info!(session = %id, %reason, "stt token refused: AI usage suspended");
            return Err(AppError::coded(
                402,
                "INSUFFICIENT_CREDITS",
                "The workspace has no AI credits left; add credits or upgrade the plan",
            ));
        }
        if !self
            .limiter
            .allow(
                &format!("copilot:stt-token:{user}"),
                STT_TOKEN_LIMIT,
                STT_TOKEN_WINDOW_SECS,
            )
            .await?
        {
            return Err(AppError::rate_limited("too many STT token requests"));
        }
        self.issuer
            .issue(SttTokenRequest {
                session: id,
                language: Some(record.language_me),
            })
            .await
            .map_err(|e| match e {
                SttIssueError::NotConfigured => AppError::coded(
                    503,
                    "STT_NOT_CONFIGURED",
                    "Speech-to-text is not configured on this server",
                ),
                SttIssueError::Upstream(m) => {
                    tracing::warn!(session = %id, error = %m, "stt token issuance failed");
                    AppError::coded(
                        502,
                        "STT_PROVIDER_ERROR",
                        "The speech-to-text provider did not issue a token",
                    )
                }
            })
    }
}
