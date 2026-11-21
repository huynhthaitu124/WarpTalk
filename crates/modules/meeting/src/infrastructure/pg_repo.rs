//! sqlx implementation of [`MeetingRepo`] (schema `meeting`, runtime queries only).
//!
//! The meeting schema carries no `workspace_id` (spec: 05a RtcSession); tenant scoping is enforced by
//! the `RoomDirectory` check that precedes every read/write path, and rows are keyed by the
//! translation room id.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, MeetingRoomId, RoomId, UserId};

use crate::application::ports::{MeetingRepo, MeetingRoom, NewParticipant};
use crate::domain::{STATUS_ACTIVE, STATUS_FINISHED, TrackChange};

pub struct PgMeetingRepo {
    pool: PgPool,
}

impl PgMeetingRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const ROOM_COLS: &str = "id, translation_room_id, provider_room_name, active_host_id, status";

fn room_from_row(r: &PgRow) -> Result<MeetingRoom, sqlx::Error> {
    Ok(MeetingRoom {
        id: MeetingRoomId::from_uuid(r.try_get("id")?),
        translation_room_id: RoomId::from_uuid(r.try_get("translation_room_id")?),
        provider_room_name: r.try_get("provider_room_name")?,
        active_host_id: r
            .try_get::<Option<Uuid>, _>("active_host_id")?
            .map(UserId::from_uuid),
        status: r.try_get("status")?,
    })
}

/// Seat row for `identity`, created (active, no timestamps) when missing. Never alters existing state.
async fn ensure_participant(
    pool: &PgPool,
    room: MeetingRoomId,
    identity: &str,
    now: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query(
        "INSERT INTO meeting.rtc_stream_participants \
           (id, meeting_room_id, user_id, provider_identity, is_active, created_at) \
         VALUES ($1, $2, $3, $4, true, $5) \
         ON CONFLICT (meeting_room_id, provider_identity) \
           DO UPDATE SET provider_identity = EXCLUDED.provider_identity \
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(room.as_uuid())
    .bind(Uuid::parse_str(identity).ok())
    .bind(identity)
    .bind(now)
    .fetch_one(pool)
    .await?;
    row.try_get("id")
}

#[async_trait]
impl MeetingRepo for PgMeetingRepo {
    async fn find_current(&self, room: RoomId) -> AppResult<Option<MeetingRoom>> {
        let row = sqlx::query(&format!(
            "SELECT {ROOM_COLS} FROM meeting.meeting_rooms \
             WHERE translation_room_id = $1 AND is_current AND deleted_at IS NULL"
        ))
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(room_from_row)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn find_by_provider_name(&self, name: &str) -> AppResult<Option<MeetingRoom>> {
        let row = sqlx::query(&format!(
            "SELECT {ROOM_COLS} FROM meeting.meeting_rooms \
             WHERE provider_room_name = $1 AND is_current AND deleted_at IS NULL"
        ))
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(room_from_row)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn insert_room(
        &self,
        room: RoomId,
        provider_room_name: &str,
        host: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO meeting.meeting_rooms \
               (id, translation_room_id, provider_room_name, status, is_current, is_active, \
                created_at, created_by) \
             VALUES ($1, $2, $3, $4, true, true, $5, $6) \
             ON CONFLICT (translation_room_id) WHERE is_current DO NOTHING",
        )
        .bind(MeetingRoomId::new().as_uuid())
        .bind(room.as_uuid())
        .bind(provider_room_name)
        .bind(STATUS_ACTIVE)
        .bind(now)
        .bind(host.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn activate(&self, id: MeetingRoomId, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE meeting.meeting_rooms \
             SET status = $2, is_active = true, ended_at = NULL, updated_at = $3 \
             WHERE id = $1 AND status <> $4 AND (status <> $2 OR NOT is_active)",
        )
        .bind(id.as_uuid())
        .bind(STATUS_ACTIVE)
        .bind(now)
        .bind(STATUS_FINISHED)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn claim_host(
        &self,
        id: MeetingRoomId,
        host: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE meeting.meeting_rooms SET active_host_id = $2, updated_at = $3 \
             WHERE id = $1 AND active_host_id IS NULL",
        )
        .bind(id.as_uuid())
        .bind(host.as_uuid())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn upsert_participant(&self, p: NewParticipant<'_>) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO meeting.rtc_stream_participants \
               (id, meeting_room_id, user_id, provider_identity, display_name, is_active, \
                created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, true, $6, $3) \
             ON CONFLICT (meeting_room_id, provider_identity) DO UPDATE \
               SET display_name = EXCLUDED.display_name, user_id = EXCLUDED.user_id, \
                   is_active = true, updated_at = $6, updated_by = $3",
        )
        .bind(Uuid::now_v7())
        .bind(p.meeting_room_id.as_uuid())
        .bind(p.user_id.as_uuid())
        .bind(p.identity)
        .bind(p.display_name)
        .bind(p.now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn try_mark_finished(&self, id: MeetingRoomId, at: DateTime<Utc>) -> AppResult<bool> {
        let r = sqlx::query(
            "UPDATE meeting.meeting_rooms \
             SET status = $2, is_active = false, ended_at = COALESCE(ended_at, $3), updated_at = $3 \
             WHERE id = $1 AND status <> $2",
        )
        .bind(id.as_uuid())
        .bind(STATUS_FINISHED)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() == 1)
    }

    async fn reopen(&self, id: MeetingRoomId, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE meeting.meeting_rooms \
             SET status = $2, is_active = true, ended_at = NULL, updated_at = $3 \
             WHERE id = $1 AND status = $4",
        )
        .bind(id.as_uuid())
        .bind(STATUS_ACTIVE)
        .bind(now)
        .bind(STATUS_FINISHED)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn participant_joined(
        &self,
        id: MeetingRoomId,
        identity: &str,
        at: DateTime<Utc>,
    ) -> AppResult<bool> {
        // Applies only if not older than the recorded join and not superseded by a later leave.
        let r = sqlx::query(
            "INSERT INTO meeting.rtc_stream_participants AS p \
               (id, meeting_room_id, user_id, provider_identity, is_active, joined_at, created_at) \
             VALUES ($1, $2, $3, $4, true, $5, $5) \
             ON CONFLICT (meeting_room_id, provider_identity) DO UPDATE \
               SET is_active = true, joined_at = $5, left_at = NULL, updated_at = $5 \
               WHERE (p.joined_at IS NULL OR p.joined_at <= $5) \
                 AND (p.left_at IS NULL OR p.left_at <= $5)",
        )
        .bind(Uuid::now_v7())
        .bind(id.as_uuid())
        .bind(Uuid::parse_str(identity).ok())
        .bind(identity)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() == 1)
    }

    async fn participant_left(
        &self,
        id: MeetingRoomId,
        identity: &str,
        at: DateTime<Utc>,
    ) -> AppResult<bool> {
        let r = sqlx::query(
            "INSERT INTO meeting.rtc_stream_participants AS p \
               (id, meeting_room_id, user_id, provider_identity, is_active, left_at, created_at) \
             VALUES ($1, $2, $3, $4, false, $5, $5) \
             ON CONFLICT (meeting_room_id, provider_identity) DO UPDATE \
               SET is_active = false, left_at = $5, updated_at = $5 \
               WHERE (p.joined_at IS NULL OR p.joined_at <= $5) \
                 AND (p.left_at IS NULL OR p.left_at <= $5)",
        )
        .bind(Uuid::now_v7())
        .bind(id.as_uuid())
        .bind(Uuid::parse_str(identity).ok())
        .bind(identity)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() == 1)
    }

    async fn clear_active_host_if(
        &self,
        id: MeetingRoomId,
        identity: &str,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE meeting.meeting_rooms SET active_host_id = NULL, updated_at = $3 \
             WHERE id = $1 AND active_host_id::text = $2",
        )
        .bind(id.as_uuid())
        .bind(identity)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn apply_track(
        &self,
        id: MeetingRoomId,
        identity: &str,
        track_sid: &str,
        media_type: &str,
        change: TrackChange,
        at: DateTime<Utc>,
    ) -> AppResult<()> {
        let participant = ensure_participant(&self.pool, id, identity, at)
            .await
            .map_err(map_sqlx_error)?;
        // One upsert per change; time guards make replays and reordering harmless.
        let sql = match change {
            TrackChange::Published => {
                "INSERT INTO meeting.meeting_tracks AS t \
                   (id, meeting_participant_id, provider_track_id, media_type, is_muted, is_active, \
                    published_at, created_at) \
                 VALUES ($1, $2, $3, $4, false, true, $5, $5) \
                 ON CONFLICT (provider_track_id) DO UPDATE \
                   SET is_active = true, unpublished_at = NULL, published_at = $5, \
                       media_type = $4, updated_at = $5 \
                   WHERE t.unpublished_at IS NULL OR t.unpublished_at <= $5"
            }
            TrackChange::Unpublished => {
                "INSERT INTO meeting.meeting_tracks AS t \
                   (id, meeting_participant_id, provider_track_id, media_type, is_muted, is_active, \
                    unpublished_at, created_at) \
                 VALUES ($1, $2, $3, $4, false, false, $5, $5) \
                 ON CONFLICT (provider_track_id) DO UPDATE \
                   SET is_active = false, unpublished_at = $5, updated_at = $5 \
                   WHERE t.published_at IS NULL OR t.published_at <= $5"
            }
            TrackChange::Muted => {
                "INSERT INTO meeting.meeting_tracks AS t \
                   (id, meeting_participant_id, provider_track_id, media_type, is_muted, is_active, \
                    created_at) \
                 VALUES ($1, $2, $3, $4, true, true, $5) \
                 ON CONFLICT (provider_track_id) DO UPDATE SET is_muted = true, updated_at = $5"
            }
            TrackChange::Unmuted => {
                "INSERT INTO meeting.meeting_tracks AS t \
                   (id, meeting_participant_id, provider_track_id, media_type, is_muted, is_active, \
                    created_at) \
                 VALUES ($1, $2, $3, $4, false, true, $5) \
                 ON CONFLICT (provider_track_id) DO UPDATE SET is_muted = false, updated_at = $5"
            }
        };
        sqlx::query(sql)
            .bind(Uuid::now_v7())
            .bind(participant)
            .bind(track_sid)
            .bind(media_type)
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn close_room(&self, id: MeetingRoomId, at: DateTime<Utc>) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE meeting.meeting_rooms \
             SET is_active = false, ended_at = COALESCE(ended_at, $2), updated_at = $2 \
             WHERE id = $1 AND is_active",
        )
        .bind(id.as_uuid())
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE meeting.rtc_stream_participants \
             SET is_active = false, left_at = COALESCE(left_at, $2), updated_at = $2 \
             WHERE meeting_room_id = $1 AND is_active",
        )
        .bind(id.as_uuid())
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE meeting.meeting_tracks SET is_active = false, \
               unpublished_at = COALESCE(unpublished_at, $2), updated_at = $2 \
             WHERE is_active AND meeting_participant_id IN \
               (SELECT id FROM meeting.rtc_stream_participants WHERE meeting_room_id = $1)",
        )
        .bind(id.as_uuid())
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }
}
