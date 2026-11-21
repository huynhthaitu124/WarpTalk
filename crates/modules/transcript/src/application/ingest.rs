//! Stream ingestion (spec 12 §5.4, §6.1, §6.2). Every entry point is idempotent on redelivery.

use std::sync::Arc;

use chrono::Duration;
use uuid::Uuid;
use warptalk_contracts::events::{SttResult, TranslateResult, TtsResult};
use warptalk_kernel::{AppError, Clock, RoomId, TranscriptId, WorkspaceId};

use super::cache::TtlCache;
use super::ports::{
    EmbeddingIndexRequest, EmbeddingPublisher, NewDubbing, NewHead, NewSegment, NewTranslation,
    PauseGate, RoomDirectory, RoomPolicy, TranscriptRepo, WorkspaceAiPolicy,
};
use crate::domain::{
    Segment, Transcript, normalize_voice_type, truncate_chars, underlying_segment_id,
};

/// Room retention cache (spec 12 §8).
const ROOM_POLICY_TTL_HOURS: i64 = 4;
/// Workspace AI policy cache (spec 12 §6.2, §8).
const WORKSPACE_POLICY_TTL_MINUTES: i64 = 5;
/// Default minimum confidence for saving a far-speaker name (spec 12 §5.4).
const FAR_SPEAKER_MIN_CONFIDENCE: f64 = 0.6;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// The message can never succeed (decode / invalid ids): dead-letter.
    #[error("invalid message: {0}")]
    Invalid(String),
    /// Infrastructure failure or a dependency that has not landed yet: retry.
    #[error("transient failure: {0}")]
    Transient(String),
}

impl From<AppError> for IngestError {
    fn from(e: AppError) -> Self {
        Self::Transient(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingested {
    /// New row(s) written.
    Stored,
    /// Redelivery: the row already existed.
    AlreadyStored,
    /// Acknowledged without persisting (empty, control marker, ephemeral room, ...).
    Ignored,
    /// Intentionally skipped because the transcript was paused when the segment was spoken.
    PausedSkip,
}

pub struct IngestService {
    repo: Arc<dyn TranscriptRepo>,
    rooms: Arc<dyn RoomDirectory>,
    room_policy: Arc<dyn RoomPolicy>,
    ai_policy: Arc<dyn WorkspaceAiPolicy>,
    pause: Arc<dyn PauseGate>,
    embeddings: Arc<dyn EmbeddingPublisher>,
    clock: Arc<dyn Clock>,
    save_cache: TtlCache<RoomId, bool>,
    ai_cache: TtlCache<WorkspaceId, (bool, bool)>,
}

impl IngestService {
    pub fn new(
        repo: Arc<dyn TranscriptRepo>,
        rooms: Arc<dyn RoomDirectory>,
        room_policy: Arc<dyn RoomPolicy>,
        ai_policy: Arc<dyn WorkspaceAiPolicy>,
        pause: Arc<dyn PauseGate>,
        embeddings: Arc<dyn EmbeddingPublisher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            rooms,
            room_policy,
            ai_policy,
            pause,
            embeddings,
            clock,
            save_cache: TtlCache::new(Duration::hours(ROOM_POLICY_TTL_HOURS)),
            ai_cache: TtlCache::new(Duration::minutes(WORKSPACE_POLICY_TTL_MINUTES)),
        }
    }

    /// Fails open (persist) when the policy cannot be read, as upstream does.
    async fn should_save(&self, room: RoomId) -> bool {
        let now = self.clock.now();
        if let Some(v) = self.save_cache.get(&room, now) {
            return v;
        }
        match self.room_policy.save_transcript(room).await {
            Ok(v) => {
                self.save_cache.put(room, v, now);
                v
            }
            Err(e) => {
                tracing::warn!(%room, error = %e, "room retention policy unavailable; persisting");
                true
            }
        }
    }

    /// Fails closed (no indexing) when the policy cannot be read.
    async fn ai_flags(&self, ws: WorkspaceId) -> Result<(bool, bool), AppError> {
        let now = self.clock.now();
        if let Some(v) = self.ai_cache.get(&ws, now) {
            return Ok(v);
        }
        let retrieval = self.ai_policy.ai_retrieval_allowed(ws).await?;
        let external = self.ai_policy.external_llm_allowed(ws).await?;
        self.ai_cache.put(ws, (retrieval, external), now);
        Ok((retrieval, external))
    }

    pub async fn ingest_stt(&self, r: SttResult) -> Result<Ingested, IngestError> {
        // Empty trailing `is_final_chunk` marker; control markers are never stored (spec 12 §5.4).
        if r.text.trim().is_empty() || r.is_control_marker() || r.language == "system" {
            return Ok(Ingested::Ignored);
        }
        let segment_id = underlying_segment_id(&r.segment_id)
            .filter(|_| r.segment_id.len() == 36)
            .ok_or_else(|| IngestError::Invalid(format!("segment_id `{}`", r.segment_id)))?;
        let room = r.meeting_id;

        if !self.should_save(room).await {
            return Ok(Ingested::Ignored);
        }
        if self.pause.is_paused(room).await.unwrap_or_else(|e| {
            tracing::warn!(%room, error = %e, "pause state unreadable; persisting segment");
            false
        }) {
            if let Err(e) = self.pause.mark_skipped(room, segment_id).await {
                tracing::warn!(%room, %segment_id, error = %e, "could not remember paused segment");
            }
            return Ok(Ingested::PausedSkip);
        }

        let now = self.clock.now();
        let transcript = self.head(room, &r.language).await?;

        let far_name = r
            .far_speaker_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .filter(|_| r.far_speaker_confidence.unwrap_or(0.0) >= FAR_SPEAKER_MIN_CONFIDENCE);
        let speaker_name = truncate_chars(far_name.unwrap_or(r.speaker_id.trim()), 100);
        let seg = NewSegment {
            id: segment_id,
            transcript_id: transcript.id,
            speaker_participant_id: Uuid::parse_str(r.speaker_id.trim()).ok(),
            speaker_name,
            original_text: r.text.clone(),
            original_language: truncate_chars(&r.language, 15),
            start_time_ms: ms(r.start_ms),
            end_time_ms: ms(r.end_ms),
            // -1.0 is the "no logprobs" sentinel (WT-277): unknown, not zero.
            confidence: r.confidence.filter(|c| *c >= 0.0).map(|c| c.min(1.0)),
            is_final: r.is_final_chunk,
            now,
        };
        if !self.repo.insert_segment(&seg).await? {
            return Ok(Ingested::AlreadyStored);
        }
        if seg.is_final {
            self.publish_embedding(&transcript, &seg).await;
        }
        Ok(Ingested::Stored)
    }

    async fn head(&self, room: RoomId, language: &str) -> Result<Transcript, IngestError> {
        if let Some(t) = self.repo.current_for_room(room).await? {
            return Ok(t);
        }
        let details = self
            .rooms
            .room(room)
            .await?
            .ok_or_else(|| IngestError::Transient(format!("room {room} not found")))?;
        let head = NewHead {
            id: TranscriptId::new(),
            workspace_id: details.workspace_id,
            room_id: room,
            source_language: truncate_chars(language, 15),
            now: self.clock.now(),
        };
        Ok(self.repo.ensure_head(&head).await?)
    }

    /// Publishes only when the workspace allows AI retrieval; failures never fail persistence.
    async fn publish_embedding(&self, transcript: &Transcript, seg: &NewSegment) {
        let result = async {
            let (retrieval, external) = self.ai_flags(transcript.workspace_id).await?;
            if !retrieval {
                return Ok(());
            }
            let ws = transcript.workspace_id;
            let chunk = serde_json::json!({
                "id": seg.id.to_string(),
                "text": format!("[{}] {}", seg.speaker_name, seg.original_text).trim().to_owned(),
                "metadata": {
                    "transcript_id": transcript.id.to_string(),
                    "translation_room_id": transcript.translation_room_id.to_string(),
                    "segment_id": seg.id.to_string(),
                    "speaker_name": seg.speaker_name,
                    "start_ms": seg.start_time_ms,
                },
            });
            let req = EmbeddingIndexRequest {
                job_id: Uuid::now_v7(),
                workspace_id: ws,
                collection_id: format!("workspace_{ws}"),
                source_type: "transcript".into(),
                source_id: transcript.id.as_uuid(),
                chunks_json: serde_json::Value::Array(vec![chunk]).to_string(),
                external_llm_allowed: external,
                ai_retrieval_allowed: true,
                retention_state: if transcript.is_current {
                    "active"
                } else {
                    "inactive"
                }
                .into(),
                deletion_state: "active".into(),
                timestamp_ms: self.clock.now().timestamp_millis(),
            };
            self.embeddings.publish(&req).await
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(segment = %seg.id, error = %e, "embedding index request not published");
        }
    }

    /// Resolve the segment of a translate/TTS result. `Ok(None)` means "intentionally skipped"
    /// (ephemeral room or paused segment); a segment that has not landed yet is transient.
    async fn target_segment(
        &self,
        raw_id: &str,
        room: Option<RoomId>,
    ) -> Result<Option<(Segment, Transcript)>, IngestError> {
        let segment_id = underlying_segment_id(raw_id)
            .ok_or_else(|| IngestError::Invalid(format!("segment_id `{raw_id}`")))?;
        if let Some(room) = room
            && !self.should_save(room).await
        {
            return Ok(None);
        }
        if let Some(found) = self.repo.segment_with_transcript(segment_id).await? {
            return Ok(Some(found));
        }
        if let Some(room) = room
            && self
                .pause
                .was_skipped(room, segment_id)
                .await
                .unwrap_or(false)
        {
            return Ok(None);
        }
        Err(IngestError::Transient(format!(
            "segment {segment_id} not stored yet"
        )))
    }

    pub async fn ingest_translation(&self, r: TranslateResult) -> Result<Ingested, IngestError> {
        if r.translated_text.trim().is_empty() || r.target_lang.trim().is_empty() {
            return Ok(Ingested::Ignored);
        }
        let Some((segment, transcript)) = self.target_segment(&r.segment_id, r.meeting_id).await?
        else {
            return Ok(Ingested::PausedSkip);
        };
        let previous = r
            .previous_translation_content_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok());
        self.repo
            .record_translation(&NewTranslation {
                segment_id: segment.id,
                workspace_id: transcript.workspace_id,
                target_language: truncate_chars(&r.target_lang, 15),
                text: r.translated_text,
                translator_model: truncate_chars(
                    r.translator_model.as_deref().unwrap_or("unknown"),
                    100,
                ),
                latency_ms: r.latency_ms.map(|v| i32::try_from(v).unwrap_or(i32::MAX)),
                is_retranslated: r.is_retranslated,
                previous_content_id: previous,
                source_stt_confidence: r
                    .source_stt_confidence
                    .filter(|c| *c >= 0.0)
                    .map(|c| c.min(1.0)),
                now: self.clock.now(),
            })
            .await?;
        Ok(Ingested::Stored)
    }

    pub async fn ingest_tts(&self, r: TtsResult) -> Result<Ingested, IngestError> {
        if r.target_lang.trim().is_empty() {
            return Ok(Ingested::Ignored);
        }
        let Some((segment, _)) = self.target_segment(&r.segment_id, r.meeting_id).await? else {
            return Ok(Ingested::PausedSkip);
        };
        let voice_type = normalize_voice_type(r.voice_type.as_deref());
        let provider = if voice_type == "cloned" {
            r.clone_provider.as_deref()
        } else {
            r.anchor_provider.as_deref()
        }
        .filter(|p| !p.is_empty())
        .unwrap_or("cartesia");
        let stored = self
            .repo
            .record_dubbing(&NewDubbing {
                segment_id: segment.id,
                target_language: truncate_chars(&r.target_lang, 15),
                voice_type: voice_type.into(),
                provider: truncate_chars(provider, 50),
                provider_voice_id: truncate_chars(
                    r.provider_voice_id
                        .as_deref()
                        .filter(|v| !v.is_empty())
                        .unwrap_or("default"),
                    255,
                ),
                duration_ms: r.duration_ms.map(|v| i32::try_from(v).unwrap_or(i32::MAX)),
                now: self.clock.now(),
            })
            .await?;
        if !stored {
            // Translation link not written yet: defer (retry) rather than lose the metadata.
            return Err(IngestError::Transient(format!(
                "no current translation for segment {} / {}",
                segment.id, r.target_lang
            )));
        }
        Ok(Ingested::Stored)
    }
}

fn ms(v: i64) -> i32 {
    i32::try_from(v.max(0)).unwrap_or(i32::MAX)
}
