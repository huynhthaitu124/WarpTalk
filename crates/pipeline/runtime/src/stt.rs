//! The speech-to-text stage contract.
//!
//! Supports both modes:
//! - **Flash / streaming**: the runtime calls [`SttSession::feed`] for every ungated live frame
//!   while the speaker talks, then [`SttSession::commit`] at utterance end. The model has
//!   already heard the audio, so commit is just "say go".
//! - **Closed chunk**: `feed` is never called; [`CommitRequest::utterance`] carries the whole
//!   audio and the session sends it at commit.
//!
//! Contract an implementation must honour:
//! 1. `commit` returns as soon as the commit is *issued*, not when the transcript is complete
//!    (otherwise the speaker task could not stream the next turn's frames meanwhile).
//! 2. Output goes to the sink given at `open_session`: zero or more `Segment`s per commit, then
//!    exactly one `UtteranceDone`, in commit order. Turn N's events must all precede turn N+1's.
//! 3. If `commit` returns `Err`, no `UtteranceDone` will be emitted for it.
//! 4. `close` finishes in-flight commits, then releases resources and drops the sink.

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::error::SttError;
use crate::messages::{ParticipantId, RoomId, SttSegment, Utterance};

/// How the STT backend consumes audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SttMode {
    /// Session buffers audio as it is fed (streaming model); commit at turn end.
    #[default]
    Realtime,
    /// Whole closed utterance sent at commit; live frames are not streamed.
    ClosedChunk,
}

#[derive(Debug, Clone)]
pub struct SessionParams {
    pub room: RoomId,
    pub speaker: ParticipantId,
    /// Declared spoken language (ISO 639-1), if known.
    pub lang_hint: Option<String>,
}

/// Output of a session towards the speaker pipeline.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Segment is the hot variant; boxing would add an alloc per segment.
pub enum SttEvent {
    Segment(SttSegment),
    /// Live text of audio not committed yet (models that transcribe as audio is appended).
    /// Display only: it may still change and is superseded by the turn's segments.
    Partial {
        text: String,
    },
    /// The commit finished (possibly with no segments, or after an error).
    UtteranceDone {
        ok: bool,
    },
}

#[derive(Debug, Clone)]
pub struct CommitRequest {
    pub utterance: Utterance,
    pub mode: SttMode,
    /// Samples (16 kHz) of this utterance already fed live. 0 means the session must send
    /// `utterance.audio` itself (closed chunk, or live streaming failed).
    pub streamed_samples: usize,
}

/// Factory of per-speaker sessions. Shared by all rooms (`Arc<dyn SpeechToText>`), so the
/// implementation owns the connection pool.
#[async_trait]
pub trait SpeechToText: Send + Sync {
    /// Opens (claims) a session. Should be near-instant when a warm connection exists.
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError>;
}

#[async_trait]
pub trait SttSession: Send {
    /// Live 16 kHz mono frames, ungated, in order (flash mode only).
    async fn feed(&mut self, pcm_16k: &[f32]) -> Result<(), SttError>;
    /// Utterance ended: transcribe it. See the module contract.
    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError>;
    /// Throw away audio fed for a turn that was gated out (too short, too quiet).
    async fn discard(&mut self) -> Result<(), SttError>;
    /// Graceful close: finish in-flight work, then release.
    async fn close(self: Box<Self>);
}
