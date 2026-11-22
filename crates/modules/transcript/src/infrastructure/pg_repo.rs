//! sqlx adapter for `TranscriptRepo` (runtime queries; schema `transcript`).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, RoomId, TranscriptId, UserId, WorkspaceId};

use crate::application::ports::{
    CorrectionOutcome, NewCorrection, NewDubbing, NewHead, NewSegment, NewTranslation,
    TranscriptRepo,
};
use crate::domain::{
    Correction, Segment, SegmentTranslation, SegmentView, Transcript, TranscriptStatus,
};

pub struct PgTranscriptRepo {
    pool: PgPool,
}

impl PgTranscriptRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const TRANSCRIPT_COLS: &str = "id, workspace_id, translation_room_id, version, status, \
    source_language, total_segments, total_duration_ms, is_current, previous_transcript_id, \
    finalized_at, created_at";

const SEGMENT_COLS: &str = "s.id, s.transcript_id, s.speaker_participant_id, s.speaker_name, \
    s.original_text, s.original_language, s.start_time_ms, s.end_time_ms, \
    s.confidence::float8 AS confidence, s.sequence_order, s.is_corrected, s.is_final";

fn transcript_from(row: &PgRow) -> AppResult<Transcript> {
    let status: String = row.try_get("status").map_err(map_sqlx_error)?;
    let g = |e| map_sqlx_error(e);
    Ok(Transcript {
        id: TranscriptId::from_uuid(row.try_get("id").map_err(g)?),
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id").map_err(g)?),
        translation_room_id: RoomId::from_uuid(row.try_get("translation_room_id").map_err(g)?),
        version: row.try_get("version").map_err(g)?,
        status: TranscriptStatus::parse(&status)
            .ok_or_else(|| AppError::internal(format!("unknown transcript status `{status}`")))?,
        source_language: row.try_get("source_language").map_err(g)?,
        total_segments: row.try_get("total_segments").map_err(g)?,
        total_duration_ms: row.try_get("total_duration_ms").map_err(g)?,
        is_current: row.try_get("is_current").map_err(g)?,
        previous_transcript_id: row
            .try_get::<Option<Uuid>, _>("previous_transcript_id")
            .map_err(g)?
            .map(TranscriptId::from_uuid),
        finalized_at: row.try_get("finalized_at").map_err(g)?,
        created_at: row.try_get("created_at").map_err(g)?,
    })
}

fn segment_from(row: &PgRow) -> AppResult<Segment> {
    let g = |e| map_sqlx_error(e);
    Ok(Segment {
        id: row.try_get("id").map_err(g)?,
        transcript_id: TranscriptId::from_uuid(row.try_get("transcript_id").map_err(g)?),
        speaker_participant_id: row.try_get("speaker_participant_id").map_err(g)?,
        speaker_name: row.try_get("speaker_name").map_err(g)?,
        original_text: row.try_get("original_text").map_err(g)?,
        original_language: row.try_get("original_language").map_err(g)?,
        start_time_ms: row.try_get("start_time_ms").map_err(g)?,
        end_time_ms: row.try_get("end_time_ms").map_err(g)?,
        confidence: row.try_get("confidence").map_err(g)?,
        sequence_order: row.try_get("sequence_order").map_err(g)?,
        is_corrected: row.try_get("is_corrected").map_err(g)?,
        is_final: row.try_get("is_final").map_err(g)?,
    })
}

fn correction_from(row: &PgRow) -> AppResult<Correction> {
    let g = |e| map_sqlx_error(e);
    Ok(Correction {
        id: row.try_get("id").map_err(g)?,
        segment_id: row.try_get("segment_id").map_err(g)?,
        user_id: UserId::from_uuid(row.try_get("user_id").map_err(g)?),
        original_text: row.try_get("original_text").map_err(g)?,
        corrected_text: row.try_get("corrected_text").map_err(g)?,
        correction_type: row.try_get("correction_type").map_err(g)?,
        status: row.try_get("status").map_err(g)?,
        reason: row.try_get("reason").map_err(g)?,
        created_at: row.try_get("created_at").map_err(g)?,
    })
}

#[async_trait]
impl TranscriptRepo for PgTranscriptRepo {
    async fn current_for_room(&self, room: RoomId) -> AppResult<Option<Transcript>> {
        let row = sqlx::query(&format!(
            "SELECT {TRANSCRIPT_COLS} FROM transcript.transcripts \
             WHERE translation_room_id = $1 AND is_current AND deleted_at IS NULL"
        ))
        .bind(room.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref().map(transcript_from).transpose()
    }

    async fn by_id(&self, id: TranscriptId) -> AppResult<Option<Transcript>> {
        let row = sqlx::query(&format!(
            "SELECT {TRANSCRIPT_COLS} FROM transcript.transcripts \
             WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref().map(transcript_from).transpose()
    }

    async fn ensure_head(&self, head: &NewHead) -> AppResult<Transcript> {
        if let Some(t) = self.current_for_room(head.room_id).await? {
            return Ok(t);
        }
        // Link to the room's most recent transcript, if any (spec 12 §5.4).
        let prev: Option<(Uuid, i32)> = sqlx::query_as(
            "SELECT id, version FROM transcript.transcripts \
             WHERE translation_room_id = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(head.room_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        // A concurrent creator wins through the unique indexes; we then read its head.
        sqlx::query(
            "INSERT INTO transcript.transcripts \
             (id, workspace_id, translation_room_id, version, status, source_language, \
              is_current, previous_transcript_id, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'RECORDING', $5, true, $6, $7, $7) \
             ON CONFLICT DO NOTHING",
        )
        .bind(head.id.as_uuid())
        .bind(head.workspace_id.as_uuid())
        .bind(head.room_id.as_uuid())
        .bind(prev.map_or(1, |(_, v)| v + 1))
        .bind(&head.source_language)
        .bind(prev.map(|(id, _)| id))
        .bind(head.now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        self.current_for_room(head.room_id)
            .await?
            .ok_or_else(|| AppError::unavailable("transcript head not yet visible"))
    }

    async fn insert_segment(&self, s: &NewSegment) -> AppResult<bool> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        // The UPDATE row-locks the transcript, serialising sequence allocation.
        let seq: i32 = sqlx::query_scalar(
            "UPDATE transcript.transcripts SET last_sequence_order = last_sequence_order + 1, \
             total_segments = total_segments + 1, \
             total_duration_ms = GREATEST(total_duration_ms, $2), updated_at = $3 \
             WHERE id = $1 RETURNING last_sequence_order",
        )
        .bind(s.transcript_id.as_uuid())
        .bind(s.end_time_ms)
        .bind(s.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let inserted = sqlx::query(
            "INSERT INTO transcript.transcript_segments \
             (id, transcript_id, speaker_participant_id, speaker_name, original_text, \
              original_language, start_time_ms, end_time_ms, confidence, sequence_order, \
              is_corrected, is_final, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::float8::numeric(5,4), $10, false, $11, $12, $12) \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(s.id)
        .bind(s.transcript_id.as_uuid())
        .bind(s.speaker_participant_id)
        .bind(&s.speaker_name)
        .bind(&s.original_text)
        .bind(&s.original_language)
        .bind(s.start_time_ms)
        .bind(s.end_time_ms)
        .bind(s.confidence)
        .bind(seq)
        .bind(s.is_final)
        .bind(s.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        if inserted == 0 {
            // Redelivery: roll back the counter advance.
            tx.rollback().await.map_err(map_sqlx_error)?;
            return Ok(false);
        }
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(true)
    }

    async fn segment_with_transcript(&self, id: Uuid) -> AppResult<Option<(Segment, Transcript)>> {
        let row = sqlx::query(&format!(
            "SELECT {SEGMENT_COLS}, \
                    t.workspace_id, t.translation_room_id, t.version, t.status, \
                    t.source_language, t.total_segments, t.total_duration_ms, t.is_current, \
                    t.previous_transcript_id, t.finalized_at, t.created_at \
             FROM transcript.transcript_segments s \
             JOIN transcript.transcripts t ON t.id = s.transcript_id \
             WHERE s.id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some(row) = row else { return Ok(None) };
        let segment = segment_from(&row)?;
        // `id` of the transcript row is the segment's transcript_id.
        let mut transcript = transcript_from_joined(&row)?;
        transcript.id = segment.transcript_id;
        Ok(Some((segment, transcript)))
    }

    async fn record_translation(&self, t: &NewTranslation) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        // Chunks of one (segment, language) may be handled by concurrent consumers: serialise.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || $2, 0))")
            .bind(t.segment_id)
            .bind(&t.target_language)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        if t.chunk_index.is_none() {
            // A translation of the whole segment replaces any sentence-level pieces.
            sqlx::query(
                "DELETE FROM transcript.translation_chunks \
                 WHERE segment_id = $1 AND target_language = $2",
            )
            .bind(t.segment_id)
            .bind(&t.target_language)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        }
        sqlx::query(
            "INSERT INTO transcript.translation_chunks \
             (segment_id, target_language, chunk_index, workspace_id, translated_text, \
              translator_model, latency_ms, source_stt_confidence, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::float8::numeric(5,4), $9, $9) \
             ON CONFLICT (segment_id, target_language, chunk_index) DO UPDATE SET \
               translated_text = EXCLUDED.translated_text, \
               translator_model = EXCLUDED.translator_model, \
               latency_ms = EXCLUDED.latency_ms, \
               source_stt_confidence = EXCLUDED.source_stt_confidence, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(t.segment_id)
        .bind(&t.target_language)
        .bind(
            t.chunk_index
                .map_or(0, |i| i32::try_from(i).unwrap_or(i32::MAX)),
        )
        .bind(t.workspace_id.as_uuid())
        .bind(&t.text)
        .bind(&t.translator_model)
        .bind(t.latency_ms)
        .bind(t.source_stt_confidence)
        .bind(t.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let pieces: Vec<String> = sqlx::query_scalar(
            "SELECT translated_text FROM transcript.translation_chunks \
             WHERE segment_id = $1 AND target_language = $2 ORDER BY chunk_index",
        )
        .bind(t.segment_id)
        .bind(&t.target_language)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let combined = warptalk_contracts::text::join_sentences(
            &t.joined_with_lang,
            pieces.iter().map(String::as_str),
        );
        let content_id: Uuid = sqlx::query_scalar(
            "INSERT INTO transcript.translation_contents \
             (id, workspace_id, text_hash, target_language, translated_text, translator_model, \
              source_stt_confidence, is_retranslated, previous_translation_content_id, \
              latency_ms, status, created_at, updated_at) \
             VALUES ($1, $2, md5($4), $3, $4, $5, $6::float8::numeric(5,4), $7, $8, $9, \
                     'completed', $10, $10) \
             ON CONFLICT (workspace_id, text_hash, target_language) \
             DO UPDATE SET updated_at = EXCLUDED.updated_at \
             RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(t.workspace_id.as_uuid())
        .bind(&t.target_language)
        .bind(&combined)
        .bind(&t.translator_model)
        .bind(t.source_stt_confidence)
        .bind(t.is_retranslated)
        .bind(t.previous_content_id)
        .bind(t.latency_ms)
        .bind(t.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE transcript.segment_translation_links SET is_current = false \
             WHERE segment_id = $1 AND target_language = $2 AND is_current \
               AND translation_content_id <> $3",
        )
        .bind(t.segment_id)
        .bind(&t.target_language)
        .bind(content_id)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO transcript.segment_translation_links \
             (segment_id, translation_content_id, target_language, is_current, delivered_at, \
              created_at, is_stale) \
             VALUES ($1, $2, $3, true, $4, $4, false) \
             ON CONFLICT (segment_id, translation_content_id) \
             DO UPDATE SET is_current = true, is_stale = false, delivered_at = EXCLUDED.delivered_at",
        )
        .bind(t.segment_id)
        .bind(content_id)
        .bind(&t.target_language)
        .bind(t.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn record_dubbing(&self, d: &NewDubbing) -> AppResult<bool> {
        let n = sqlx::query(
            "INSERT INTO transcript.audio_dubbings \
             (id, workspace_id, translation_content_id, text_hash, voice_type, provider, \
              provider_voice_id, duration_ms, status, created_at, updated_at) \
             SELECT $1, c.workspace_id, c.id, c.text_hash, $4, $5, $6, $7, 'completed', $8, $8 \
             FROM transcript.segment_translation_links l \
             JOIN transcript.translation_contents c ON c.id = l.translation_content_id \
             WHERE l.segment_id = $2 AND l.target_language = $3 AND l.is_current \
             ON CONFLICT (workspace_id, text_hash, provider_voice_id) \
             DO UPDATE SET duration_ms = EXCLUDED.duration_ms, updated_at = EXCLUDED.updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(d.segment_id)
        .bind(&d.target_language)
        .bind(&d.voice_type)
        .bind(&d.provider)
        .bind(&d.provider_voice_id)
        .bind(d.duration_ms)
        .bind(d.now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        Ok(n > 0)
    }

    async fn list_segments(
        &self,
        transcript: TranscriptId,
        lang: Option<&str>,
    ) -> AppResult<Vec<SegmentView>> {
        let rows = sqlx::query(&format!(
            "SELECT {SEGMENT_COLS}, c.translated_text, l.target_language AS tr_lang, \
                    l.is_stale AS tr_stale \
             FROM transcript.transcript_segments s \
             LEFT JOIN transcript.segment_translation_links l \
                    ON l.segment_id = s.id AND l.is_current AND l.target_language = $2 \
             LEFT JOIN transcript.translation_contents c ON c.id = l.translation_content_id \
             WHERE s.transcript_id = $1 \
             ORDER BY s.sequence_order"
        ))
        .bind(transcript.as_uuid())
        .bind(lang)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(|row| {
                let text: Option<String> =
                    row.try_get("translated_text").map_err(map_sqlx_error)?;
                let translation = match text {
                    Some(text) => Some(SegmentTranslation {
                        language: row.try_get("tr_lang").map_err(map_sqlx_error)?,
                        text,
                        is_stale: row.try_get("tr_stale").map_err(map_sqlx_error)?,
                    }),
                    None => None,
                };
                Ok(SegmentView {
                    segment: segment_from(row)?,
                    translation,
                })
            })
            .collect()
    }

    async fn add_correction(&self, c: &NewCorrection) -> AppResult<CorrectionOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        // FOR SHARE: a concurrent finalize waits for us; a finished finalize is seen here.
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM transcript.transcripts \
             WHERE id = $1 AND deleted_at IS NULL FOR SHARE",
        )
        .bind(c.transcript_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let Some(status) = status else {
            return Ok(CorrectionOutcome::SegmentNotFound);
        };
        if TranscriptStatus::parse(&status).is_none_or(TranscriptStatus::is_locked) {
            return Ok(CorrectionOutcome::Locked);
        }
        let original: Option<String> = sqlx::query_scalar(
            "SELECT original_text FROM transcript.transcript_segments \
             WHERE id = $1 AND transcript_id = $2 FOR UPDATE",
        )
        .bind(c.segment_id)
        .bind(c.transcript_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let Some(original) = original else {
            return Ok(CorrectionOutcome::SegmentNotFound);
        };
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO transcript.transcript_corrections \
             (id, segment_id, user_id, original_text, corrected_text, correction_type, status, \
              triggered_retranslation, reviewed_by, reviewed_at, created_at, reason) \
             VALUES ($1, $2, $3, $4, $5, 'STT', 'APPROVED', false, $3, $6, $6, $7)",
        )
        .bind(id)
        .bind(c.segment_id)
        .bind(c.user_id.as_uuid())
        .bind(&original)
        .bind(&c.text)
        .bind(c.now)
        .bind(&c.reason)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE transcript.transcript_segments SET original_text = $2, is_corrected = true, \
             clean_text = NULL, clean_flags = NULL, updated_at = $3 WHERE id = $1",
        )
        .bind(c.segment_id)
        .bind(&c.text)
        .bind(c.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        // The source text changed: current translations no longer match it (WT-704).
        sqlx::query(
            "UPDATE transcript.segment_translation_links SET is_stale = true \
             WHERE segment_id = $1 AND is_current",
        )
        .bind(c.segment_id)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(CorrectionOutcome::Created(Correction {
            id,
            segment_id: c.segment_id,
            user_id: c.user_id,
            original_text: original,
            corrected_text: c.text.clone(),
            correction_type: "STT".into(),
            status: "APPROVED".into(),
            reason: c.reason.clone(),
            created_at: c.now,
        }))
    }

    async fn list_corrections(
        &self,
        transcript: TranscriptId,
        segment: Option<Uuid>,
    ) -> AppResult<Vec<Correction>> {
        let rows = sqlx::query(
            "SELECT c.id, c.segment_id, c.user_id, c.original_text, c.corrected_text, \
                    c.correction_type, c.status, c.reason, c.created_at \
             FROM transcript.transcript_corrections c \
             JOIN transcript.transcript_segments s ON s.id = c.segment_id \
             WHERE s.transcript_id = $1 AND ($2::uuid IS NULL OR c.segment_id = $2) \
             ORDER BY c.created_at, c.id",
        )
        .bind(transcript.as_uuid())
        .bind(segment)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter().map(correction_from).collect()
    }

    async fn finalize(&self, id: TranscriptId, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE transcript.transcripts SET status = 'FINALIZED', is_active = false, \
             finalized_at = $2, updated_at = $2 \
             WHERE id = $1 AND status IN ('RECORDING', 'FINALIZING')",
        )
        .bind(id.as_uuid())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }
}

/// Transcript columns of a join where `id` belongs to another table (set by the caller).
fn transcript_from_joined(row: &PgRow) -> AppResult<Transcript> {
    let status: String = row.try_get("status").map_err(map_sqlx_error)?;
    let g = |e| map_sqlx_error(e);
    Ok(Transcript {
        id: TranscriptId::new(),
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id").map_err(g)?),
        translation_room_id: RoomId::from_uuid(row.try_get("translation_room_id").map_err(g)?),
        version: row.try_get("version").map_err(g)?,
        status: TranscriptStatus::parse(&status)
            .ok_or_else(|| AppError::internal(format!("unknown transcript status `{status}`")))?,
        source_language: row.try_get("source_language").map_err(g)?,
        total_segments: row.try_get("total_segments").map_err(g)?,
        total_duration_ms: row.try_get("total_duration_ms").map_err(g)?,
        is_current: row.try_get("is_current").map_err(g)?,
        previous_transcript_id: row
            .try_get::<Option<Uuid>, _>("previous_transcript_id")
            .map_err(g)?
            .map(TranscriptId::from_uuid),
        finalized_at: row.try_get("finalized_at").map_err(g)?,
        created_at: row.try_get("created_at").map_err(g)?,
    })
}
