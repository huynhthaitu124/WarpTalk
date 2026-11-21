//! sqlx implementation of [`RoomRepo`]. Mutations lock the room row (`FOR UPDATE`) in one
//! transaction; routes are reconciled in the same transaction.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use warptalk_contracts::events::{
    AudioRoute, AudioRoutesSnapshot, RoomCommand, RoomStartedParticipant, RoomStartedState,
    RouteStatus,
};
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{
    AppError, AppResult, Page, PageRequest, ParticipantId, RoomId, UserId, WorkspaceId,
};

use crate::application::ports::{
    Caller, ClaimOutcome, Heartbeat, JoinInput, Mutation, NewBridgeRoom, NewRoom,
    ParticipantRecord, RoomRecord, RoomRepo, VoiceConsentDirectory,
};
use crate::domain::{
    self, AudioMode, BridgeConfig, BridgeProvider, BridgeState, CapturerLease, DomainError,
    ExistingRoute, ParticipantStatus, ROOM_TYPE_EXTERNAL_BRIDGE, RoomAction, RoomStatus, RoomType,
    RouteSpeaker, STAND_IN_DISPLAY_NAME, STAND_IN_USER_ID, plan_routes, room_languages,
};

/// Name of the partial unique index "one open bridge room per workspace + provider + meeting".
pub const OPEN_BRIDGE_INDEX: &str = "ux_translation_rooms_open_bridge_meeting";
pub const OPEN_BRIDGE_CONFLICT: &str = "An open room for this external meeting already exists.";

/// `true` when `e` is a violation of the open-bridge-room unique index.
pub fn is_open_bridge_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db)
        if db.code().as_deref() == Some("23505") && db.constraint() == Some(OPEN_BRIDGE_INDEX))
}

fn map_bridge_insert_error(e: sqlx::Error) -> AppError {
    if is_open_bridge_violation(&e) {
        AppError::conflict("BRIDGE_ROOM_EXISTS", OPEN_BRIDGE_CONFLICT)
    } else {
        map_sqlx_error(e)
    }
}

pub fn domain_error(e: DomainError) -> AppError {
    match e {
        DomainError::RoomClosed | DomainError::InvalidTransition { .. } => {
            AppError::conflict("INVALID_STATE", e.to_string())
        }
        DomainError::Kicked | DomainError::Rejected => AppError::forbidden(e.to_string()),
    }
}

pub struct PgRoomRepo {
    pool: PgPool,
    consent: Arc<dyn VoiceConsentDirectory>,
}

impl PgRoomRepo {
    pub fn new(pool: PgPool, consent: Arc<dyn VoiceConsentDirectory>) -> Self {
        Self { pool, consent }
    }
}

const ROOM_COLS: &str = "r.id, r.workspace_id, r.host_id, r.active_host_id, r.title, \
    r.translation_room_code, r.status, r.source_language, r.target_languages, r.settings, \
    r.scheduled_at, r.started_at, r.ended_at, r.created_at, \
    EXISTS (SELECT 1 FROM translation_room.translation_room_sessions s \
            WHERE s.translation_room_id = r.id AND s.status = 'ACTIVE') AS has_active_session, \
    r.translation_room_type, r.bridge_provider, r.bridge_meeting_code, r.bridge_join_url, \
    r.bridge_capturer_user_id, r.bridge_capturer_lease_at, \
    r.bridge_audio_flowing, r.bridge_last_audio_at, r.bridge_audio_mode, r.bridge_epoch";

#[derive(sqlx::FromRow)]
struct RoomRow {
    id: Uuid,
    workspace_id: Uuid,
    host_id: Uuid,
    active_host_id: Option<Uuid>,
    title: String,
    translation_room_code: String,
    status: String,
    source_language: String,
    target_languages: serde_json::Value,
    settings: serde_json::Value,
    scheduled_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    has_active_session: bool,
    translation_room_type: String,
    bridge_provider: Option<String>,
    bridge_meeting_code: Option<String>,
    bridge_join_url: Option<String>,
    bridge_capturer_user_id: Option<Uuid>,
    bridge_capturer_lease_at: Option<DateTime<Utc>>,
    bridge_audio_flowing: Option<bool>,
    bridge_last_audio_at: Option<DateTime<Utc>>,
    bridge_audio_mode: String,
    bridge_epoch: i32,
}

impl RoomRow {
    fn into_record(self) -> AppResult<RoomRecord> {
        let status = RoomStatus::parse(&self.status)
            .ok_or_else(|| AppError::internal(format!("unknown room status {}", self.status)))?;
        let targets: Vec<String> =
            serde_json::from_value(self.target_languages).unwrap_or_default();
        let room_type = RoomType::parse(&self.translation_room_type);
        let bridge = if room_type == RoomType::ExternalBridge {
            let provider = self
                .bridge_provider
                .as_deref()
                .and_then(BridgeProvider::parse)
                .ok_or_else(|| AppError::internal("bridge room without a known provider"))?;
            Some(BridgeState {
                provider,
                meeting_code: self.bridge_meeting_code.clone().unwrap_or_default(),
                join_url: self.bridge_join_url.clone(),
                lease: CapturerLease {
                    user_id: self.bridge_capturer_user_id,
                    lease_at: self.bridge_capturer_lease_at,
                    audio_flowing: self.bridge_audio_flowing,
                    last_audio_at: self.bridge_last_audio_at,
                },
                audio_mode: AudioMode::parse(&self.bridge_audio_mode).unwrap_or(AudioMode::Voice),
                epoch: self.bridge_epoch,
            })
        } else {
            None
        };
        Ok(RoomRecord {
            id: self.id.into(),
            workspace_id: self.workspace_id.into(),
            host_id: self.host_id.into(),
            active_host_id: self.active_host_id.map(Into::into),
            title: self.title,
            code: self.translation_room_code,
            status,
            source_language: self.source_language,
            target_languages: targets,
            requires_approval: self
                .settings
                .get("requires_approval")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            scheduled_at: self.scheduled_at,
            started_at: self.started_at,
            ended_at: self.ended_at,
            created_at: self.created_at,
            translation_active: status == RoomStatus::InProgress && self.has_active_session,
            room_type,
            bridge,
        })
    }
}

#[derive(sqlx::FromRow)]
struct PartRow {
    id: Uuid,
    user_id: Option<Uuid>,
    display_name: String,
    role: String,
    listen_language: String,
    speak_language: String,
    status: String,
    is_using_voice_clone: bool,
    joined_at: Option<DateTime<Utc>>,
    left_at: Option<DateTime<Utc>>,
}

const PART_COLS: &str = "id, user_id, display_name, role, listen_language, speak_language, \
    status, is_using_voice_clone, joined_at, left_at";

impl PartRow {
    fn into_record(self) -> AppResult<ParticipantRecord> {
        let status = ParticipantStatus::parse(&self.status).ok_or_else(|| {
            AppError::internal(format!("unknown participant status {}", self.status))
        })?;
        Ok(ParticipantRecord {
            id: self.id.into(),
            user_id: self.user_id.map(Into::into),
            display_name: self.display_name,
            is_host: self.role.eq_ignore_ascii_case("host"),
            speak_language: self.speak_language,
            listen_language: self.listen_language,
            status,
            is_using_voice_clone: self.is_using_voice_clone,
            joined_at: self.joined_at,
            left_at: self.left_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RouteRow {
    id: Uuid,
    source_participant_id: Uuid,
    target_participant_id: Uuid,
    source_language: String,
    target_language: String,
    voice_clone_enabled: bool,
    status: String,
}

fn route_status(s: &str) -> RouteStatus {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).unwrap_or(RouteStatus::Pending)
}

fn route_status_str(s: RouteStatus) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

async fn fetch_room(conn: &mut PgConnection, id: RoomId) -> AppResult<Option<RoomRecord>> {
    let sql = format!(
        "SELECT {ROOM_COLS} FROM translation_room.translation_rooms r \
         WHERE r.id = $1 AND r.deleted_at IS NULL"
    );
    sqlx::query_as::<_, RoomRow>(&sql)
        .bind(id.as_uuid())
        .fetch_optional(conn)
        .await
        .map_err(map_sqlx_error)?
        .map(RoomRow::into_record)
        .transpose()
}

async fn lock_room(conn: &mut PgConnection, id: RoomId) -> AppResult<RoomRecord> {
    let sql = format!(
        "SELECT {ROOM_COLS} FROM translation_room.translation_rooms r \
         WHERE r.id = $1 AND r.deleted_at IS NULL FOR UPDATE OF r"
    );
    sqlx::query_as::<_, RoomRow>(&sql)
        .bind(id.as_uuid())
        .fetch_optional(conn)
        .await
        .map_err(map_sqlx_error)?
        .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?
        .into_record()
}

async fn participants_of(
    conn: &mut PgConnection,
    room: RoomId,
) -> AppResult<Vec<ParticipantRecord>> {
    let sql = format!(
        "SELECT {PART_COLS} FROM translation_room.translation_room_participants \
         WHERE translation_room_id = $1 ORDER BY created_at, id"
    );
    sqlx::query_as::<_, PartRow>(&sql)
        .bind(room.as_uuid())
        .fetch_all(conn)
        .await
        .map_err(map_sqlx_error)?
        .into_iter()
        .map(PartRow::into_record)
        .collect()
}

async fn participant_by_id(
    conn: &mut PgConnection,
    room: RoomId,
    id: ParticipantId,
) -> AppResult<ParticipantRecord> {
    let sql = format!(
        "SELECT {PART_COLS} FROM translation_room.translation_room_participants \
         WHERE translation_room_id = $1 AND id = $2"
    );
    sqlx::query_as::<_, PartRow>(&sql)
        .bind(room.as_uuid())
        .bind(id.as_uuid())
        .fetch_optional(conn)
        .await
        .map_err(map_sqlx_error)?
        .ok_or_else(|| AppError::not_found("participant not found"))?
        .into_record()
}

async fn participant_by_user(
    conn: &mut PgConnection,
    room: RoomId,
    user: UserId,
) -> AppResult<Option<ParticipantRecord>> {
    let sql = format!(
        "SELECT {PART_COLS} FROM translation_room.translation_room_participants \
         WHERE translation_room_id = $1 AND user_id = $2"
    );
    sqlx::query_as::<_, PartRow>(&sql)
        .bind(room.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(conn)
        .await
        .map_err(map_sqlx_error)?
        .map(PartRow::into_record)
        .transpose()
}

async fn build_snapshot(
    conn: &mut PgConnection,
    room: &RoomRecord,
    now: DateTime<Utc>,
) -> AppResult<AudioRoutesSnapshot> {
    let rows = sqlx::query_as::<_, RouteRow>(
        "SELECT id, source_participant_id, target_participant_id, source_language, \
                target_language, voice_clone_enabled, status \
         FROM translation_room.translation_room_audio_routes \
         WHERE translation_room_id = $1 AND is_current \
         ORDER BY source_participant_id, target_participant_id",
    )
    .bind(room.id.as_uuid())
    .fetch_all(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(AudioRoutesSnapshot {
        generated_at: now,
        room_status: room.status.as_str().to_owned(),
        translation_active: room.translation_active,
        room_languages: room_languages(&room.source_language, &room.target_languages),
        routes: rows
            .into_iter()
            .map(|r| AudioRoute {
                route_id: r.id,
                source_participant_id: r.source_participant_id.into(),
                target_participant_id: r.target_participant_id.into(),
                source_language: r.source_language,
                target_language: r.target_language,
                voice_clone_enabled: r.voice_clone_enabled,
                status: route_status(&r.status),
            })
            .collect(),
    })
}

async fn close_all_routes(
    conn: &mut PgConnection,
    room: RoomId,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE translation_room.translation_room_audio_routes \
         SET is_current = false, ended_at = $2, status = 'COMPLETED' \
         WHERE translation_room_id = $1 AND is_current",
    )
    .bind(room.as_uuid())
    .bind(now)
    .execute(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

/// Which route work `finish` performs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Snap {
    /// Neither reconcile nor snapshot.
    Skip,
    /// Reconcile and snapshot, only while the room is IN_PROGRESS.
    Reconcile,
    /// Snapshot without reconciling.
    Always,
}

impl PgRoomRepo {
    /// Bring the current routes in line with the CONNECTED roster (spec: 10 §3.7).
    async fn reconcile_routes(
        &self,
        conn: &mut PgConnection,
        room: RoomId,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let connected: Vec<ParticipantRecord> = participants_of(&mut *conn, room)
            .await?
            .into_iter()
            .filter(|p| p.status == ParticipantStatus::Connected)
            .collect();
        let mut consent: HashMap<UserId, bool> = HashMap::new();
        for p in &connected {
            if let Some(u) = p.user_id
                && p.is_using_voice_clone
                && !consent.contains_key(&u)
            {
                consent.insert(u, self.consent.has_voice_clone_consent(u).await?);
            }
        }
        let speakers: Vec<RouteSpeaker> = connected
            .iter()
            .map(|p| RouteSpeaker {
                id: p.id,
                speak_language: p.speak_language.clone(),
                listen_language: p.listen_language.clone(),
                voice_clone: p.is_using_voice_clone
                    && p.user_id
                        .is_some_and(|u| consent.get(&u).copied().unwrap_or(false)),
            })
            .collect();
        let existing: Vec<ExistingRoute> = sqlx::query_as::<_, RouteRow>(
            "SELECT id, source_participant_id, target_participant_id, source_language, \
                    target_language, voice_clone_enabled, status \
             FROM translation_room.translation_room_audio_routes \
             WHERE translation_room_id = $1 AND is_current",
        )
        .bind(room.as_uuid())
        .fetch_all(&mut *conn)
        .await
        .map_err(map_sqlx_error)?
        .into_iter()
        .map(|r| ExistingRoute {
            id: r.id,
            source: r.source_participant_id.into(),
            target: r.target_participant_id.into(),
            source_language: r.source_language,
            target_language: r.target_language,
            voice_clone: r.voice_clone_enabled,
        })
        .collect();

        let plan = plan_routes(&speakers, &existing);
        for id in &plan.remove {
            sqlx::query(
                "UPDATE translation_room.translation_room_audio_routes \
                 SET is_current = false, ended_at = $2, status = 'COMPLETED' WHERE id = $1",
            )
            .bind(id)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(map_sqlx_error)?;
        }
        for (id, clone) in &plan.set_clone {
            sqlx::query(
                "UPDATE translation_room.translation_room_audio_routes \
                 SET voice_clone_enabled = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(clone)
            .execute(&mut *conn)
            .await
            .map_err(map_sqlx_error)?;
        }
        for r in &plan.add {
            sqlx::query(
                "INSERT INTO translation_room.translation_room_audio_routes \
                 (id, translation_room_id, source_participant_id, target_participant_id, \
                  source_language, target_language, voice_clone_enabled, status, is_current) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true)",
            )
            .bind(Uuid::now_v7())
            .bind(room.as_uuid())
            .bind(r.source.as_uuid())
            .bind(r.target.as_uuid())
            .bind(&r.source_language)
            .bind(&r.target_language)
            .bind(r.voice_clone)
            .bind(route_status_str(RouteStatus::Pending))
            .execute(&mut *conn)
            .await
            .map_err(map_sqlx_error)?;
        }
        Ok(())
    }

    /// Reconcile when the room is live, then re-read the room and build the mutation result.
    async fn finish(
        &self,
        conn: &mut PgConnection,
        room: RoomId,
        participant: Option<ParticipantRecord>,
        commands: Vec<RoomCommand>,
        snap: Snap,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation> {
        let reconcile = snap == Snap::Reconcile;
        let force_snapshot = snap == Snap::Always;
        let rec = fetch_room(&mut *conn, room)
            .await?
            .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
        let live = rec.status == RoomStatus::InProgress;
        if reconcile && live {
            self.reconcile_routes(&mut *conn, room, now).await?;
        }
        let snapshot = if (reconcile && live) || force_snapshot {
            Some(build_snapshot(&mut *conn, &rec, now).await?)
        } else {
            None
        };
        Ok(Mutation {
            room: rec,
            participant,
            commands,
            snapshot,
        })
    }
}

/// Host check. Non-host callers with any relation to the room get Forbidden, outsiders NotFound.
async fn authorize_host(
    conn: &mut PgConnection,
    room: &RoomRecord,
    caller: Caller,
    allow_ws_admin: bool,
) -> AppResult<()> {
    if caller.user == room.effective_host() || (allow_ws_admin && caller.is_ws_admin()) {
        return Ok(());
    }
    let related = caller.ws_role.is_some()
        || participant_by_user(conn, room.id, caller.user)
            .await?
            .is_some();
    if related {
        Err(AppError::forbidden("Only the host can perform this action"))
    } else {
        Err(AppError::not_found("TranslationRoom not found"))
    }
}

fn ensure_open(room: &RoomRecord) -> AppResult<()> {
    if room.status.is_terminal() || room.status == RoomStatus::Failed {
        return Err(domain_error(DomainError::RoomClosed));
    }
    Ok(())
}

fn started_state(room: &RoomRecord, parts: &[ParticipantRecord]) -> RoomStartedState {
    RoomStartedState {
        room_id: room.id,
        code: room.code.clone(),
        status: RoomStatus::InProgress.as_str().to_owned(),
        participants: parts
            .iter()
            .filter(|p| p.status == ParticipantStatus::Connected)
            .filter_map(|p| {
                Some(RoomStartedParticipant {
                    user_id: p.user_id?.as_uuid(),
                    display_name: p.display_name.clone(),
                    speak_language: p.speak_language.clone(),
                    listen_language: p.listen_language.clone(),
                    is_muted: false,
                    joined_at: p.joined_at?,
                })
            })
            .collect(),
    }
}

fn bridge_of(room: &RoomRecord) -> AppResult<&BridgeState> {
    room.bridge.as_ref().ok_or_else(|| {
        AppError::conflict(
            "NOT_BRIDGE_ROOM",
            "This room is not an external bridge room.",
        )
    })
}

/// Make `user` the capturer: new lease, epoch + 1, audio assumed flowing until reported otherwise.
async fn assign_capturer(
    conn: &mut PgConnection,
    room: RoomId,
    user: UserId,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE translation_room.translation_rooms SET bridge_capturer_user_id = $2, \
         bridge_capturer_since = $3, bridge_capturer_lease_at = $3, bridge_audio_flowing = true, \
         bridge_last_audio_at = $3, bridge_epoch = bridge_epoch + 1, updated_at = $3, updated_by = $2 \
         WHERE id = $1",
    )
    .bind(room.as_uuid())
    .bind(user.as_uuid())
    .bind(now)
    .execute(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn refresh_lease(conn: &mut PgConnection, room: RoomId, now: DateTime<Utc>) -> AppResult<()> {
    sqlx::query(
        "UPDATE translation_room.translation_rooms SET bridge_capturer_lease_at = $2 WHERE id = $1",
    )
    .bind(room.as_uuid())
    .bind(now)
    .execute(conn)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

#[async_trait]
impl RoomRepo for PgRoomRepo {
    async fn create(&self, new: NewRoom) -> AppResult<RoomRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let settings = serde_json::json!({ "requires_approval": new.requires_approval });
        sqlx::query(
            "INSERT INTO translation_room.translation_rooms \
             (id, workspace_id, host_id, title, translation_room_code, status, source_language, \
              target_languages, settings, scheduled_at, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $3)",
        )
        .bind(new.id.as_uuid())
        .bind(new.workspace_id.as_uuid())
        .bind(new.host.as_uuid())
        .bind(&new.title)
        .bind(&new.code)
        .bind(new.status.as_str())
        .bind(&new.source_language)
        .bind(serde_json::json!(new.target_languages))
        .bind(settings)
        .bind(new.scheduled_at)
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO translation_room.translation_room_participants \
             (id, translation_room_id, user_id, display_name, role, listen_language, \
              speak_language, status, joined_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'host', $5, $5, 'CONNECTED', $6, $6, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(new.id.as_uuid())
        .bind(new.host.as_uuid())
        .bind(&new.host_display_name)
        .bind(&new.source_language)
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let rec = fetch_room(&mut tx, new.id)
            .await?
            .ok_or_else(|| AppError::internal("room vanished after insert"))?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(rec)
    }

    async fn list(
        &self,
        workspace: WorkspaceId,
        status: Option<RoomStatus>,
        page: &PageRequest,
    ) -> AppResult<Page<RoomRecord>> {
        let status = status.map(|s| s.as_str());
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM translation_room.translation_rooms \
             WHERE workspace_id = $1 AND deleted_at IS NULL AND ($2::text IS NULL OR status = $2)",
        )
        .bind(workspace.as_uuid())
        .bind(status)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let sql = format!(
            "SELECT {ROOM_COLS} FROM translation_room.translation_rooms r \
             WHERE r.workspace_id = $1 AND r.deleted_at IS NULL AND ($2::text IS NULL OR r.status = $2) \
             ORDER BY r.created_at DESC, r.id DESC LIMIT $3 OFFSET $4"
        );
        let rows = sqlx::query_as::<_, RoomRow>(&sql)
            .bind(workspace.as_uuid())
            .bind(status)
            .bind(page.limit())
            .bind(page.offset())
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        let items = rows
            .into_iter()
            .map(RoomRow::into_record)
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Page::new(items, page, total))
    }

    async fn get(&self, id: RoomId) -> AppResult<Option<RoomRecord>> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        fetch_room(&mut conn, id).await
    }

    async fn find_by_code(&self, code: &str) -> AppResult<Option<RoomRecord>> {
        let sql = format!(
            "SELECT {ROOM_COLS} FROM translation_room.translation_rooms r \
             WHERE r.translation_room_code = $1 AND r.deleted_at IS NULL \
             ORDER BY r.created_at DESC LIMIT 1"
        );
        sqlx::query_as::<_, RoomRow>(&sql)
            .bind(code)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?
            .map(RoomRow::into_record)
            .transpose()
    }

    async fn has_participant(&self, room: RoomId, user: UserId) -> AppResult<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM translation_room.translation_room_participants \
             WHERE translation_room_id = $1 AND user_id = $2 \
             AND status NOT IN ('KICKED', 'REJECTED'))",
        )
        .bind(room.as_uuid())
        .bind(user.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)
    }

    async fn participants(&self, room: RoomId) -> AppResult<Vec<ParticipantRecord>> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        participants_of(&mut conn, room).await
    }

    async fn routes_snapshot(
        &self,
        room: RoomId,
        now: DateTime<Utc>,
    ) -> AppResult<AudioRoutesSnapshot> {
        let mut conn = self.pool.acquire().await.map_err(map_sqlx_error)?;
        let rec = fetch_room(&mut conn, room)
            .await?
            .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
        build_snapshot(&mut conn, &rec, now).await
    }

    async fn lifecycle(
        &self,
        room: RoomId,
        caller: Caller,
        action: RoomAction,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        authorize_host(&mut tx, &rec, caller, action == RoomAction::End).await?;
        let from = rec.status;
        let to = domain::transition(from, action).map_err(domain_error)?;
        let rid = room.as_uuid();

        if from == to && action == RoomAction::End {
            // already ENDED: idempotent success
            let m = self
                .finish(&mut tx, room, None, vec![], Snap::Skip, now)
                .await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            return Ok(m);
        }

        sqlx::query(
            "UPDATE translation_room.translation_rooms SET status = $2, updated_at = $3, \
             updated_by = $4, \
             started_at = CASE WHEN $2 = 'IN_PROGRESS' THEN COALESCE(started_at, $3) ELSE started_at END, \
             ended_at = CASE WHEN $2 IN ('ENDED', 'CANCELLED') THEN $3 ELSE ended_at END, \
             duration_seconds = CASE WHEN $2 = 'ENDED' AND started_at IS NOT NULL \
                  THEN EXTRACT(EPOCH FROM ($3 - started_at))::int ELSE duration_seconds END \
             WHERE id = $1",
        )
        .bind(rid)
        .bind(to.as_str())
        .bind(now)
        .bind(caller.user.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;

        // Sessions (spec: 10 §3.6).
        match action {
            RoomAction::Start | RoomAction::Resume => {
                let active: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM translation_room.translation_room_sessions \
                     WHERE translation_room_id = $1 AND status = 'ACTIVE')",
                )
                .bind(rid)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                if !active {
                    sqlx::query(
                        "INSERT INTO translation_room.translation_room_sessions \
                         (id, translation_room_id, main_language, status, started_at, created_at, updated_at) \
                         VALUES ($1, $2, $3, 'ACTIVE', $4, $4, $4)",
                    )
                    .bind(Uuid::now_v7())
                    .bind(rid)
                    .bind(&rec.source_language)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                }
            }
            RoomAction::Pause
            | RoomAction::StopTranslation
            | RoomAction::End
            | RoomAction::Cancel => {
                sqlx::query(
                    "UPDATE translation_room.translation_room_sessions \
                     SET status = 'ENDED', ended_at = $2, updated_at = $2 \
                     WHERE translation_room_id = $1 AND status <> 'ENDED'",
                )
                .bind(rid)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            }
            RoomAction::OpenWaiting | RoomAction::Expire => {}
        }

        let closing = matches!(action, RoomAction::End | RoomAction::Cancel);
        if closing {
            sqlx::query(
                "UPDATE translation_room.translation_room_participants \
                 SET status = 'DISCONNECTED', left_at = $2, updated_at = $2 \
                 WHERE translation_room_id = $1 AND status = 'CONNECTED'",
            )
            .bind(rid)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            close_all_routes(&mut tx, room, now).await?;
        }

        let mut commands = Vec::new();
        match action {
            RoomAction::Start if from != RoomStatus::InProgress => {
                let parts = participants_of(&mut tx, room).await?;
                commands.push(RoomCommand::RoomStarted {
                    room_id: room,
                    state: started_state(&rec, &parts),
                });
            }
            RoomAction::StopTranslation => {
                commands.push(RoomCommand::TranslationStopped { room_id: room })
            }
            RoomAction::End => commands.push(RoomCommand::RoomEnded { room_id: room }),
            RoomAction::Cancel => commands.push(RoomCommand::CancelRoom { room_id: room }),
            _ => {}
        }
        let snap = match action {
            RoomAction::OpenWaiting | RoomAction::Expire => Snap::Skip,
            RoomAction::Start | RoomAction::Resume => Snap::Reconcile,
            _ => Snap::Always,
        };
        let m = self
            .finish(&mut tx, room, None, commands, snap, now)
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }

    async fn join(&self, input: JoinInput) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, input.room).await?;
        ensure_open(&rec)?;
        let langs = room_languages(&rec.source_language, &rec.target_languages);
        if !langs.contains(&input.speak_language) || !langs.contains(&input.listen_language) {
            return Err(AppError::validation(
                "LANGUAGE_NOT_IN_ROOM",
                "speakLanguage and listenLanguage must be languages of the room",
            ));
        }
        let existing = participant_by_user(&mut tx, input.room, input.user).await?;
        let is_host = input.user == rec.effective_host() || input.user == rec.host_id;
        let new_status = domain::join_status(
            existing.as_ref().map(|p| p.status),
            is_host,
            rec.requires_approval,
        )
        .map_err(domain_error)?;
        let now = input.now;
        let participant_id = match &existing {
            Some(p) => {
                let name = input
                    .display_name
                    .clone()
                    .unwrap_or_else(|| p.display_name.clone());
                sqlx::query(
                    "UPDATE translation_room.translation_room_participants \
                     SET display_name = $2, speak_language = $3, listen_language = $4, status = $5, \
                         joined_at = CASE WHEN $5 = 'CONNECTED' AND status <> 'CONNECTED' THEN $6 \
                                          ELSE joined_at END, \
                         left_at = CASE WHEN $5 = 'CONNECTED' THEN NULL ELSE left_at END, \
                         updated_at = $6 \
                     WHERE id = $1",
                )
                .bind(p.id.as_uuid())
                .bind(&name)
                .bind(&input.speak_language)
                .bind(&input.listen_language)
                .bind(new_status.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                p.id
            }
            None => {
                let id = ParticipantId::new();
                sqlx::query(
                    "INSERT INTO translation_room.translation_room_participants \
                     (id, translation_room_id, user_id, display_name, role, listen_language, \
                      speak_language, status, joined_at, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
                             CASE WHEN $8 = 'CONNECTED' THEN $9 END, $9, $9)",
                )
                .bind(id.as_uuid())
                .bind(input.room.as_uuid())
                .bind(input.user.as_uuid())
                .bind(input.display_name.as_deref().unwrap_or("Participant"))
                .bind(if is_host { "host" } else { "participant" })
                .bind(&input.listen_language)
                .bind(&input.speak_language)
                .bind(new_status.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                id
            }
        };
        let mut commands = Vec::new();
        if new_status == ParticipantStatus::Waiting
            && existing.as_ref().map(|p| p.status) != Some(ParticipantStatus::Waiting)
        {
            commands.push(RoomCommand::ParticipantWaiting {
                room_id: input.room,
                user_id: Some(input.user.as_uuid()),
            });
        }
        let participant = participant_by_id(&mut tx, input.room, participant_id).await?;
        let m = self
            .finish(
                &mut tx,
                input.room,
                Some(participant),
                commands,
                Snap::Reconcile,
                now,
            )
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }

    async fn admit(
        &self,
        room: RoomId,
        participant: ParticipantId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        authorize_host(&mut tx, &rec, caller, true).await?;
        ensure_open(&rec)?;
        let p = participant_by_id(&mut tx, room, participant).await?;
        if p.status != ParticipantStatus::Waiting {
            return Err(AppError::conflict(
                "INVALID_STATE",
                "Participant is not waiting for admission",
            ));
        }
        sqlx::query(
            "UPDATE translation_room.translation_room_participants \
             SET status = 'CONNECTED', joined_at = COALESCE(joined_at, $2), left_at = NULL, \
                 updated_at = $2 WHERE id = $1",
        )
        .bind(participant.as_uuid())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let cmd = RoomCommand::ParticipantAdmitted {
            room_id: room,
            user_id: p.user_id.map(|u| u.as_uuid()),
        };
        let updated = participant_by_id(&mut tx, room, participant).await?;
        let m = self
            .finish(
                &mut tx,
                room,
                Some(updated),
                vec![cmd],
                Snap::Reconcile,
                now,
            )
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }

    async fn kick(
        &self,
        room: RoomId,
        participant: ParticipantId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        authorize_host(&mut tx, &rec, caller, false).await?;
        ensure_open(&rec)?;
        let p = participant_by_id(&mut tx, room, participant).await?;
        if p.is_host || p.user_id == Some(rec.effective_host()) {
            return Err(AppError::forbidden("Cannot kick the host."));
        }
        if p.status == ParticipantStatus::Kicked {
            return Err(AppError::conflict(
                "ALREADY_KICKED",
                "Participant has already been removed from this room",
            ));
        }
        sqlx::query(
            "UPDATE translation_room.translation_room_participants \
             SET status = 'KICKED', left_at = $2, updated_at = $2 WHERE id = $1",
        )
        .bind(participant.as_uuid())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let cmd = RoomCommand::Kick {
            room_id: room,
            user_id: p.user_id.map(|u| u.as_uuid()),
        };
        let updated = participant_by_id(&mut tx, room, participant).await?;
        let m = self
            .finish(
                &mut tx,
                room,
                Some(updated),
                vec![cmd],
                Snap::Reconcile,
                now,
            )
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }

    async fn leave(&self, room: RoomId, user: UserId, now: DateTime<Utc>) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let _ = lock_room(&mut tx, room).await?;
        let p = participant_by_user(&mut tx, room, user)
            .await?
            .ok_or_else(|| AppError::not_found("participant not found"))?;
        let Some(next) = domain::leave_status(p.status) else {
            let m = self
                .finish(&mut tx, room, Some(p), vec![], Snap::Skip, now)
                .await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            return Ok(m);
        };
        sqlx::query(
            "UPDATE translation_room.translation_room_participants \
             SET status = $2, left_at = $3, updated_at = $3 WHERE id = $1",
        )
        .bind(p.id.as_uuid())
        .bind(next.as_str())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let updated = participant_by_id(&mut tx, room, p.id).await?;
        let m = self
            .finish(&mut tx, room, Some(updated), vec![], Snap::Reconcile, now)
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }

    async fn generate_routes(
        &self,
        room: RoomId,
        caller: Caller,
        now: DateTime<Utc>,
    ) -> AppResult<Mutation> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        authorize_host(&mut tx, &rec, caller, true).await?;
        ensure_open(&rec)?;
        let connected = participants_of(&mut tx, room)
            .await?
            .into_iter()
            .filter(|p| p.status == ParticipantStatus::Connected)
            .count();
        if connected == 0 {
            return Err(AppError::validation(
                "NO_PARTICIPANTS_IN_ROOM",
                "No participants in the room",
            ));
        }
        self.reconcile_routes(&mut tx, room, now).await?;
        let m = self
            .finish(&mut tx, room, None, vec![], Snap::Always, now)
            .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(m)
    }
    async fn claim_bridge(
        &self,
        new: NewBridgeRoom,
        _cfg: BridgeConfig,
    ) -> AppResult<ClaimOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        // Serialise concurrent claims of the same meeting so the loser sees the winner's row.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "bridge:{}:{}:{}",
                new.workspace_id,
                new.provider.as_str(),
                new.meeting_code
            ))
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM translation_room.translation_rooms \
             WHERE workspace_id = $1 AND translation_room_type = $2 AND bridge_provider = $3 \
               AND bridge_meeting_code = $4 AND deleted_at IS NULL \
               AND status NOT IN ('ENDED', 'CANCELLED', 'EXPIRED', 'FAILED')",
        )
        .bind(new.workspace_id.as_uuid())
        .bind(ROOM_TYPE_EXTERNAL_BRIDGE)
        .bind(new.provider.as_str())
        .bind(&new.meeting_code)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;

        if let Some(id) = existing {
            let id = RoomId::from(id);
            let rec = lock_room(&mut tx, id).await?;
            let lease = &bridge_of(&rec)?.lease;
            let is_capturer = match lease.user_id {
                None => {
                    assign_capturer(&mut tx, id, new.creator, new.now).await?;
                    true
                }
                Some(u) if u == new.creator.as_uuid() => {
                    refresh_lease(&mut tx, id, new.now).await?;
                    true
                }
                Some(_) => false,
            };
            let room = fetch_room(&mut tx, id)
                .await?
                .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
            tx.commit().await.map_err(map_sqlx_error)?;
            return Ok(ClaimOutcome {
                room,
                created: false,
                is_capturer,
            });
        }

        sqlx::query(
            "INSERT INTO translation_room.translation_rooms \
             (id, workspace_id, host_id, title, translation_room_code, status, translation_room_type, \
              max_participants, source_language, target_languages, settings, created_at, created_by, \
              bridge_provider, bridge_meeting_code, bridge_join_url) \
             VALUES ($1, $2, $3, $4, $5, 'WAITING', $6, 20, $7, $8, $9, $10, $3, $11, $12, $13)",
        )
        .bind(new.id.as_uuid())
        .bind(new.workspace_id.as_uuid())
        .bind(new.creator.as_uuid())
        .bind(&new.title)
        .bind(&new.code)
        .bind(ROOM_TYPE_EXTERNAL_BRIDGE)
        .bind(&new.source_language)
        .bind(serde_json::json!(new.target_languages))
        .bind(serde_json::json!({ "requires_approval": false }))
        .bind(new.now)
        .bind(new.provider.as_str())
        .bind(&new.meeting_code)
        .bind(&new.join_url)
        .execute(&mut *tx)
        .await
        .map_err(map_bridge_insert_error)?;
        for (user, name, role, conn_type) in [
            (
                new.creator.as_uuid(),
                new.creator_display_name.as_str(),
                "host",
                "webrtc",
            ),
            (STAND_IN_USER_ID, STAND_IN_DISPLAY_NAME, "bridge", "bridge"),
        ] {
            sqlx::query(
                "INSERT INTO translation_room.translation_room_participants \
                 (id, translation_room_id, user_id, display_name, role, listen_language, \
                  speak_language, status, connection_type, joined_at, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $6, 'CONNECTED', $7, $8, $8, $8)",
            )
            .bind(Uuid::now_v7())
            .bind(new.id.as_uuid())
            .bind(user)
            .bind(name)
            .bind(role)
            .bind(&new.source_language)
            .bind(conn_type)
            .bind(new.now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        }
        assign_capturer(&mut tx, new.id, new.creator, new.now).await?;
        let room = fetch_room(&mut tx, new.id)
            .await?
            .ok_or_else(|| AppError::internal("room vanished after insert"))?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(ClaimOutcome {
            room,
            created: true,
            is_capturer: true,
        })
    }

    async fn bridge_heartbeat(
        &self,
        room: RoomId,
        user: UserId,
        hb: Heartbeat,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        let state = bridge_of(&rec)?;
        ensure_open(&rec)?;
        if state.lease.user_id != Some(user.as_uuid()) {
            return Err(AppError::conflict(
                "CAPTURER_LEASE_LOST",
                "You are not the capturer of this bridge room.",
            ));
        }
        let reported = hb.last_audio_at.map(|t| t.min(now));
        let last_audio = if hb.audio_flowing {
            Some(reported.unwrap_or(now))
        } else {
            reported.or(state.lease.last_audio_at)
        };
        sqlx::query(
            "UPDATE translation_room.translation_rooms SET bridge_capturer_lease_at = $2, \
             bridge_audio_flowing = $3, bridge_last_audio_at = $4 WHERE id = $1",
        )
        .bind(room.as_uuid())
        .bind(now)
        .bind(hb.audio_flowing)
        .bind(last_audio)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let out = fetch_room(&mut tx, room)
            .await?
            .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(out)
    }

    async fn bridge_takeover(
        &self,
        room: RoomId,
        caller: Caller,
        cfg: BridgeConfig,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        let state = bridge_of(&rec)?;
        ensure_open(&rec)?;
        let me = caller.user.as_uuid();
        if state.lease.user_id == Some(me) {
            refresh_lease(&mut tx, room, now).await?;
        } else {
            let is_host = caller.user == rec.effective_host() || caller.user == rec.host_id;
            if !(is_host || state.lease.user_id.is_none() || state.lease.is_stale(now, &cfg)) {
                return Err(AppError::conflict(
                    "CAPTURER_ACTIVE",
                    "Another participant is currently capturing this meeting.",
                ));
            }
            assign_capturer(&mut tx, room, caller.user, now).await?;
        }
        let out = fetch_room(&mut tx, room)
            .await?
            .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(out)
    }

    async fn set_bridge_audio_mode(
        &self,
        room: RoomId,
        caller: Caller,
        mode: AudioMode,
        now: DateTime<Utc>,
    ) -> AppResult<RoomRecord> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rec = lock_room(&mut tx, room).await?;
        let state = bridge_of(&rec)?;
        ensure_open(&rec)?;
        let allowed = caller.user == rec.effective_host()
            || caller.user == rec.host_id
            || state.lease.user_id == Some(caller.user.as_uuid());
        if !allowed {
            return Err(AppError::forbidden(
                "Only the host or the capturer can change the audio mode",
            ));
        }
        if mode == AudioMode::Voice && state.audio_mode == AudioMode::Text && rec.translation_active
        {
            return Err(AppError::conflict(
                "BRIDGE_AUDIO_MODE_LOCKED",
                "Text-only mode cannot return to voice while translation is running.",
            ));
        }
        sqlx::query(
            "UPDATE translation_room.translation_rooms SET bridge_audio_mode = $2, \
             updated_at = $3, updated_by = $4 WHERE id = $1",
        )
        .bind(room.as_uuid())
        .bind(mode.as_str())
        .bind(now)
        .bind(caller.user.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let out = fetch_room(&mut tx, room)
            .await?
            .ok_or_else(|| AppError::not_found("TranslationRoom not found"))?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(out)
    }
}
