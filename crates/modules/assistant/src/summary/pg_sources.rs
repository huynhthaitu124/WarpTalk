//! Read-only Postgres adapters of [`TranscriptSource`] and [`RoomMetaSource`].
//!
//! TEMPORARY cross-schema reads of `transcript.*` and `translation_room.*` (runtime SQL), like
//! the other worker adapters; replace with ports implemented by those modules.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, WorkspaceId};

use super::ports::{
    CopilotNames, RoomContext, RoomMeta, RoomMetaSource, TranscriptData, TranscriptSegment,
    TranscriptSource,
};
use warptalk_kernel::UserId;

/// Final transcript segments for the summary job. TEMPORARY cross-schema read of
/// `transcript.*` like the adapters above (read-only).
pub struct PgTranscriptSource {
    pool: PgPool,
}

impl PgTranscriptSource {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TranscriptSource for PgTranscriptSource {
    async fn load(&self, room: RoomId, limit: u32) -> AppResult<Option<TranscriptData>> {
        let head: Option<(Uuid, Uuid, i32, String)> = sqlx::query_as(
            "SELECT id, workspace_id, version, status FROM transcript.transcripts \
             WHERE translation_room_id = $1 AND is_current AND deleted_at IS NULL",
        )
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some((id, ws, version, status)) = head else {
            return Ok(None);
        };
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transcript.transcript_segments \
             WHERE transcript_id = $1 AND is_final",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let rows: Vec<(Uuid, String, String, String, i32)> = if limit == 0 {
            Vec::new()
        } else {
            sqlx::query_as(
                "SELECT id, speaker_name, original_text, original_language, start_time_ms \
                 FROM transcript.transcript_segments \
                 WHERE transcript_id = $1 AND is_final AND btrim(original_text) <> '' \
                 ORDER BY sequence_order LIMIT $2",
            )
            .bind(id)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?
        };
        Ok(Some(TranscriptData {
            workspace_id: WorkspaceId::from_uuid(ws),
            version,
            finalized: matches!(status.as_str(), "FINALIZED" | "ARCHIVED"),
            segments: rows
                .into_iter()
                .map(|(id, speaker, text, language, start)| TranscriptSegment {
                    id,
                    speaker,
                    text,
                    language,
                    start_ms: i64::from(start),
                })
                .collect(),
            total_segments: u32::try_from(total).unwrap_or(u32::MAX),
        }))
    }
}

/// Room title, languages and participant names for the summary prompt. TEMPORARY cross-schema
/// read of `translation_room.*` like the adapters above.
pub struct PgRoomMetaSource {
    pool: PgPool,
}

impl PgRoomMetaSource {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RoomMetaSource for PgRoomMetaSource {
    async fn room(&self, room: RoomId) -> AppResult<Option<RoomMeta>> {
        let row: Option<(Uuid, String, String, serde_json::Value)> = sqlx::query_as(
            "SELECT workspace_id, title, source_language, target_languages \
             FROM translation_room.translation_rooms WHERE id = $1",
        )
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some((ws, title, source_language, targets)) = row else {
            return Ok(None);
        };
        let participants: Vec<String> = sqlx::query_scalar(
            "SELECT display_name FROM translation_room.translation_room_participants \
             WHERE translation_room_id = $1 ORDER BY created_at",
        )
        .bind(room.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(Some(RoomMeta {
            workspace_id: WorkspaceId::from_uuid(ws),
            title,
            source_language,
            target_languages: serde_json::from_value(targets).unwrap_or_default(),
            participants,
        }))
    }

    async fn context(&self, room: RoomId) -> AppResult<Option<RoomContext>> {
        let row: Option<(Uuid, String, String, Option<Uuid>)> = sqlx::query_as(
            "SELECT host_id, title, translation_room_type, series_id \
             FROM translation_room.translation_rooms WHERE id = $1",
        )
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some((host, title, room_type, series_id)) = row else {
            return Ok(None);
        };
        let host_name: Option<String> =
            sqlx::query_scalar("SELECT full_name FROM identity.users WHERE id = $1")
                .bind(host)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
        let copilot = if room_type == "COPILOT_SESSION" {
            let names: Option<(String, String)> = sqlx::query_as(
                "SELECT name_me, name_others FROM copilot.sessions WHERE room_id = $1",
            )
            .bind(room.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
            Some(match names {
                Some((me, others)) => CopilotNames { me, others },
                None => CopilotNames {
                    me: "Tôi".into(),
                    others: "Người khác".into(),
                },
            })
        } else {
            None
        };
        let users: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT display_name, user_id FROM translation_room.translation_room_participants \
             WHERE translation_room_id = $1 AND user_id IS NOT NULL ORDER BY created_at",
        )
        .bind(room.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(Some(RoomContext {
            host_user_id: Some(UserId::from_uuid(host)),
            host_name: host_name.filter(|n| !n.trim().is_empty()),
            copilot,
            series_id,
            title,
            participant_users: users
                .into_iter()
                .map(|(n, u)| (n, UserId::from_uuid(u)))
                .collect(),
        }))
    }
}
