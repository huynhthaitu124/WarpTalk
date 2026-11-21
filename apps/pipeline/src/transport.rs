//! Media transport abstraction: how the pipeline host gets participant audio in and puts dub
//! audio / captions out. The LiveKit RTC SDK cannot be built everywhere, so the host only
//! knows these traits. Implementations: [`crate::simulated::SimulatedTransport`] (in-process,
//! tests, local runs, benchmarking) and `livekit_transport::LiveKitTransport` (feature
//! `livekit-rtc`).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc;
use warptalk_audio::Frame;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::ParticipantId;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("join failed: {0}")]
    Join(String),
    #[error("media session closed")]
    Closed,
    #[error("publish failed: {0}")]
    Publish(String),
}

/// A block of mono `f32` audio of one participant (any block length; 48 kHz or 16 kHz is
/// typical). `captured_at` is the monotonic instant it reached this process.
#[derive(Debug, Clone)]
pub struct ParticipantAudio {
    pub participant: ParticipantId,
    pub frame: Frame,
    pub captured_at: Instant,
}

#[derive(Debug, Clone)]
pub enum MediaEvent {
    ParticipantJoined {
        participant: ParticipantId,
        at: Instant,
    },
    ParticipantLeft {
        participant: ParticipantId,
        at: Instant,
    },
    Audio(ParticipantAudio),
}

/// A chunk of dub audio to put on the per-language published track.
#[derive(Debug, Clone)]
pub struct PcmChunk {
    pub samples: Arc<[f32]>,
    pub sample_rate_hz: u32,
}

/// Joins rooms as the hidden pipeline agent.
#[async_trait]
pub trait MediaTransport: Send + Sync {
    /// Joins `room` under `identity`. Audio of every participant already present or joining
    /// later arrives on the session's event channel.
    async fn join(&self, room: RoomId, identity: &str) -> Result<Box<dyn MediaSession>, MediaError>;
}

/// One joined room. Shared (`Arc`) between the room worker (events) and the downstream stage
/// (publishing), hence `&self` everywhere.
#[async_trait]
pub trait MediaSession: Send + Sync {
    /// The event stream; yields `Some` exactly once. Transports must never back-pressure the
    /// media thread: when the channel is full, audio is dropped and counted instead.
    fn take_events(&self) -> Option<mpsc::Receiver<MediaEvent>>;

    /// Publishes dub audio for `target_lang` / `voice` on that language's published track
    /// (created on first use).
    async fn publish_dub(
        &self,
        target_lang: &str,
        voice: &str,
        chunk: PcmChunk,
    ) -> Result<(), MediaError>;

    /// Publishes a reliable data-channel message (captions JSON) under `topic`.
    async fn publish_data(&self, topic: &str, payload: Vec<u8>) -> Result<(), MediaError>;

    /// Leaves the room and releases tracks. Idempotent.
    async fn close(&self);
}

/// Counter of audio blocks a transport had to drop because the pipeline did not keep up.
pub const MEDIA_EVENTS_DROPPED_METRIC: &str = "warptalk_pipeline_media_events_dropped_total";
