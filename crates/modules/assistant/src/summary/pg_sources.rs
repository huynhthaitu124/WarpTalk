//! Read-only Postgres adapter of [`TranscriptSource`].
//!
//! TEMPORARY cross-schema read of `transcript.*` (runtime SQL): the transcript module has no
//! public query facade yet. Room, identity, copilot and workspace facts go through the ports in
//! [`super::composed`], implemented in `apps/*` over the owning modules' query APIs.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, RoomId, WorkspaceId};

use super::ports::{TranscriptData, TranscriptSegment, TranscriptSource};

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
        self.load_ordered(room, limit, false).await
    }

    async fn load_recent(&self, room: RoomId, limit: u32) -> AppResult<Option<TranscriptData>> {
        self.load_ordered(room, limit, true).await
    }
}

impl PgTranscriptSource {
    /// `newest`: keep the newest `limit` segments instead of the oldest (both oldest first).
    async fn load_ordered(
        &self,
        room: RoomId,
        limit: u32,
        newest: bool,
    ) -> AppResult<Option<TranscriptData>> {
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
            let sql = if newest {
                "SELECT id, speaker_name, original_text, original_language, start_time_ms FROM ( \
                     SELECT id, speaker_name, original_text, original_language, start_time_ms, \
                            sequence_order \
                     FROM transcript.transcript_segments \
                     WHERE transcript_id = $1 AND is_final AND btrim(original_text) <> '' \
                     ORDER BY sequence_order DESC LIMIT $2) newest \
                 ORDER BY sequence_order"
            } else {
                "SELECT id, speaker_name, original_text, original_language, start_time_ms \
                 FROM transcript.transcript_segments \
                 WHERE transcript_id = $1 AND is_final AND btrim(original_text) <> '' \
                 ORDER BY sequence_order LIMIT $2"
            };
            sqlx::query_as(sql)
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
