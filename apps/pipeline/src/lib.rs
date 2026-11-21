//! Realtime plane host: LiveKit audio -> VAD -> STT -> downstream stage, one actor per room
//! (ADR-2). The library holds everything testable without LiveKit; `main.rs` wires it.
//!
//! # Layout
//!
//! - [`transport`]: `MediaTransport` / `MediaSession` traits (audio in, dub + captions out).
//! - [`simulated`]: in-process transport with real-time pacing and capture of what was published.
//! - `livekit_transport` (feature `livekit-rtc`, off by default): the LiveKit implementation.
//! - [`worker`]: one room: media events -> frame chunker -> `RoomPipeline` -> [`downstream`].
//! - [`downstream`]: `DownstreamStage` (MT/TTS plug in here) and the default `SttOnlyStage`.
//! - [`host`]: Redis lease ownership, command/scan discovery, routes updates, shutdown.
//! - [`bench`]: the latency harness behind the `pipeline-bench` binary.
//!
//! # Latency notes
//!
//! - The audio path from a transport event to `SpeakerHandle::push` does no I/O and no await;
//!   transports must drop (and count) audio instead of blocking when the worker lags.
//! - Lease renewal, routes updates and speaker removal run off the audio loop.
//! - Captions go to the room data channel and `stt:results` concurrently; neither waits for
//!   the other. The per-meeting stream copy is written concurrently with the global one.
//! - Metrics under `warptalk_stage_latency_ms`: runtime stages `endpoint_decision`,
//!   `stt_first_text`, `stt_final` plus this crate's `stt_result_publish`,
//!   `e2e_first_caption`, `e2e_final_caption` (speech end to caption published).

pub mod bench;
pub mod config;
pub mod downstream;
pub mod fakes;
pub mod host;
#[cfg(feature = "livekit-rtc")]
pub mod livekit_transport;
pub mod prewarm;
pub mod results;
pub mod routes;
pub mod simulated;
pub mod transport;
pub mod tuning;
pub mod wav;
pub mod worker;

pub use downstream::{
    DownstreamContext, DownstreamStage, SttOnlyStage, TranslateDubDeps, TranslateDubStage,
};
pub use host::{HostConfig, HostDeps, PipelineHost, StartOutcome};
pub use results::{MemoryResultSink, RedisResultSink, ResultSink};
pub use routes::RoutesView;
pub use simulated::{SimParticipant, SimulatedTransport};
pub use transport::{MediaEvent, MediaSession, MediaTransport, PcmChunk};
pub use worker::{RoomWorker, RoomWorkerDeps};
