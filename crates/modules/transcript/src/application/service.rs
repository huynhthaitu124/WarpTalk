//! REST use cases: read, correct, finalize (spec 12 §3, §5.1-5.3).

use std::sync::Arc;

use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, TranscriptId, UserId};

use super::ports::{
    AccessTarget, CorrectionOutcome, NewCorrection, RoomHost, TranscriptReadAccess, TranscriptRepo,
};
use crate::domain::{Correction, FinalizeAction, SegmentView, Transcript};

pub const MAX_CORRECTION_CHARS: usize = 10_000;
pub const MAX_REASON_CHARS: usize = 500;

pub struct TranscriptDetail {
    pub transcript: Transcript,
    pub segments: Vec<SegmentView>,
}

pub struct TranscriptService {
    repo: Arc<dyn TranscriptRepo>,
    host: Arc<dyn RoomHost>,
    access: Arc<dyn TranscriptReadAccess>,
    clock: Arc<dyn Clock>,
}

impl TranscriptService {
    pub fn new(
        repo: Arc<dyn TranscriptRepo>,
        host: Arc<dyn RoomHost>,
        access: Arc<dyn TranscriptReadAccess>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            host,
            access,
            clock,
        }
    }

    /// The effective host is always admitted; others go through the read-access port.
    async fn authorize_read(&self, user: UserId, t: &Transcript) -> AppResult<()> {
        if self.host.effective_host(t.translation_room_id).await? == Some(user) {
            return Ok(());
        }
        let target = AccessTarget {
            workspace_id: t.workspace_id,
            room_id: t.translation_room_id,
        };
        if self.access.can_read(user, target).await? {
            Ok(())
        } else {
            Err(AppError::forbidden("no access to this transcript"))
        }
    }

    async fn load(&self, id: TranscriptId) -> AppResult<Transcript> {
        self.repo
            .by_id(id)
            .await?
            .ok_or_else(|| AppError::not_found("transcript not found"))
    }

    /// Current transcript of a room with segments ordered by sequence; translations for `lang`.
    pub async fn get_for_room(
        &self,
        user: UserId,
        room: RoomId,
        lang: Option<&str>,
    ) -> AppResult<TranscriptDetail> {
        let transcript = self
            .repo
            .current_for_room(room)
            .await?
            .ok_or_else(|| AppError::not_found("transcript not found"))?;
        self.authorize_read(user, &transcript).await?;
        let lang = lang.map(str::trim).filter(|l| !l.is_empty());
        let segments = self.repo.list_segments(transcript.id, lang).await?;
        Ok(TranscriptDetail {
            transcript,
            segments,
        })
    }

    /// Submit an STT correction. History is kept as a separate record; the segment text is updated.
    pub async fn correct(
        &self,
        user: UserId,
        transcript_id: TranscriptId,
        segment_id: Uuid,
        text: &str,
        reason: Option<&str>,
    ) -> AppResult<Correction> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AppError::validation("VALIDATION_ERROR", "text is required"));
        }
        if text.chars().count() > MAX_CORRECTION_CHARS {
            return Err(AppError::validation("VALIDATION_ERROR", "text is too long"));
        }
        let reason = reason.map(str::trim).filter(|r| !r.is_empty());
        if reason.is_some_and(|r| r.chars().count() > MAX_REASON_CHARS) {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                "reason is too long",
            ));
        }
        let transcript = self.load(transcript_id).await?;
        self.authorize_read(user, &transcript).await?;
        if transcript.status.is_locked() {
            return Err(locked());
        }
        let outcome = self
            .repo
            .add_correction(&NewCorrection {
                transcript_id,
                segment_id,
                user_id: user,
                text: text.to_owned(),
                reason: reason.map(str::to_owned),
                now: self.clock.now(),
            })
            .await?;
        match outcome {
            CorrectionOutcome::Created(c) => Ok(c),
            CorrectionOutcome::Locked => Err(locked()),
            CorrectionOutcome::SegmentNotFound => {
                Err(AppError::not_found("segment not found in this transcript"))
            }
        }
    }

    pub async fn corrections(
        &self,
        user: UserId,
        transcript_id: TranscriptId,
        segment: Option<Uuid>,
    ) -> AppResult<Vec<Correction>> {
        let transcript = self.load(transcript_id).await?;
        self.authorize_read(user, &transcript).await?;
        self.repo.list_corrections(transcript_id, segment).await
    }

    /// Host only. Repeating on a FINALIZED transcript is a success no-op.
    pub async fn finalize(&self, user: UserId, transcript_id: TranscriptId) -> AppResult<()> {
        let transcript = self.load(transcript_id).await?;
        let host = self
            .host
            .effective_host(transcript.translation_room_id)
            .await?
            .ok_or_else(|| AppError::not_found("room not found"))?;
        if host != user {
            return Err(AppError::forbidden("only the host can finalize the transcript"));
        }
        match transcript.status.finalize()? {
            FinalizeAction::AlreadyDone => Ok(()),
            FinalizeAction::Proceed => self.repo.finalize(transcript_id, self.clock.now()).await,
        }
    }
}

fn locked() -> AppError {
    AppError::validation(
        "TRANSCRIPT_LOCKED",
        "a finalized transcript cannot be corrected",
    )
}
