//! Realtime plane host: LiveKit audio -> VAD -> STT -> MT -> TTS -> dub, one actor per room
//! (ADR-2). The library holds everything testable without LiveKit; `main.rs` wires it.
//!
//! # Layout
//!
//! - [`transport`]: `MediaTransport` / `MediaSession` traits (audio in, dub + captions out).
//! - [`simulated`]: in-process transport with real-time pacing and capture of what was published.
//! - `livekit_transport` (feature `livekit-rtc`, off by default): the LiveKit implementation.
//! - [`worker`]: one room: media events -> frame chunker -> `RoomPipeline` -> [`downstream`].
//! - [`downstream`]: `DownstreamStage`; `SttOnlyStage` (captions + `stt:results`) and
//!   `TranslateDubStage` (STT -> MT -> TTS -> dub, `translate:results`, `tts:results`,
//!   captions with translations, route-degraded system events).
//! - [`tuning`]: the `mt` / `tts` / `latency` keys of the `pipeline` settings namespace and the
//!   latency flags (early endpoint, early MT, TTS pre-warm); [`prewarm`]: the TTS pre-warm
//!   decorator; [`fakes`]: MT/TTS doubles with the spec 08 latency profile.
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
//! - In the chain a segment reaches MT before its caption is written, a translation reaches TTS
//!   before its caption is written, and dub chunks are published the moment TTS yields them.
//! - On stop the downstream stage drains (bounded) BEFORE the media session closes, so the
//!   last translation and the tail of the dub are not lost.
//! - Metrics under `warptalk_stage_latency_ms`: runtime stages `endpoint_decision`,
//!   `stt_first_text`, `stt_final` plus this crate's `stt_result_publish`,
//!   `e2e_first_caption`, `e2e_final_caption` (speech end to caption published); chain stages
//!   `mt_first_token`, `mt_done`, `tts_first_audio` and `e2e_first_translated_caption`,
//!   `e2e_first_dub_audio`, `e2e_last_dub_audio` (speech end to dub audio published).
//!   Numbers and the optimizations behind them: `docs/perf/README.md`.

pub mod bench;
pub mod config;
pub mod downstream;
pub mod fakes;
pub mod glossary;
pub mod host;
#[cfg(feature = "livekit-rtc")]
pub mod livekit_transport;
pub mod prewarm;
pub mod results;
pub mod routes;
pub mod simulated;
pub mod transport;
pub mod tuning;
pub mod voice_state;
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
