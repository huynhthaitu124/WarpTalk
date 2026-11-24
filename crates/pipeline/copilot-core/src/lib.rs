//! On-device copilot core: the Rust engine the macOS app embeds (ADR 0001 items 3-5).
//!
//! ```text
//! push_audio(me | others, f32 mono, any rate)      (sync, never blocks; bounded drop-oldest queue)
//!   -> resample 16 kHz -> EnergyVad + Endpointer (per-channel config, hot-reloadable)
//!   -> streaming STT session (flash mode: live frames while speaking, commit at utterance end)
//!   -> SegmentRecord (early sentences first) -> CopilotEvent + SegmentSink (HTTP, batched)
//! ```
//!
//! * [`CopilotEngine`] (async, Rust-native) and [`CopilotHandle`] (sync facade with its own
//!   runtime; what UniFFI exposes).
//! * STT sits behind `warptalk_pipeline_runtime::SpeechToText`: [`EphemeralRealtimeStt`]
//!   (OpenAI Realtime, server-issued ephemeral token via [`SttTokenProvider`]), [`FakeStt`]
//!   (tests / offline). WhisperKit (P2) would be another implementation.
//! * Sessions are rotated, and the token refreshed ahead of expiry, only at utterance
//!   boundaries, so a reconnect never cuts speech.
//! * Echo guard (P0, see [`echo`]): marks `me` utterances dominated by `others` as
//!   `likely_echo`. Full AEC is P2.
//! * Times in [`SegmentRecord`] are offsets from session start (the moment the first audio of
//!   the channel was pushed; `start_offset_ms` shifts a channel that began later). Early
//!   sentences get estimated sub-ranges of the utterance (the model returns no timestamps).
//!
//! # UniFFI surface (cargo feature `ffi`, default off)
//!
//! Proc-macro form (`#[uniffi::export]` etc.); every type below is owned, non-generic and
//! lifetime-free already:
//!
//! ```text
//! enum Channel { Me, Others }
//! enum PushStatus { Accepted, DroppedOldest, Closed, Ignored }
//! enum StatusKind { Started, Stopped, SpeechStart, SpeechEnd, SttConnected, SttUnavailable,
//!                   SinkOnline, SinkOffline, AudioDropped }
//! record SegmentRecord { client_segment_id: String, channel: Channel, text: String,
//!                        language: String, start_ms: u64, end_ms: u64, confidence: Option<f32>,
//!                        is_final: bool, likely_echo: bool, utterance_seq: u64 }
//! enum CopilotEvent { Partial{channel,text}, Segment{segment}, Status{channel,kind,detail},
//!                     Error{channel,code,message}, Levels{channel,rms} }   // data-carrying enum
//! record StartParams { base_url, session_id, bearer }
//! record EngineStats / ChannelStats / SinkStats     // plain integer fields
//! error  CopilotError (flat enum, String payloads)
//! object CopilotHandle { constructor new(config_json: String) throws;
//!                        start(StartParams) throws; push_audio(Channel, Vec<f32>, u32) -> PushStatus;
//!                        update_endpointer(Channel, json: String) throws; update_bearer(String);
//!                        stats() throws; stop() throws; events() -> EventStream }
//! object EventStream { next(timeout_ms: u64) -> Option<CopilotEvent>; drain(max: u32) -> Vec<CopilotEvent> }
//! ```
//!
//! Config crosses the boundary as a JSON string (`CopilotConfig::from_json`, all fields
//! optional) so the Swift side needs no mirror of the large endpointer struct. A callback
//! interface (`trait EventListener { fn on_event(CopilotEvent) }`) can replace `EventStream`
//! polling if the app prefers push; `push_audio` should be called from the audio callback's
//! own thread (it only takes a short mutex).

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

mod channel;
pub mod config;
pub mod echo;
pub mod engine;
pub mod error;
pub mod handle;
pub mod queue;
pub mod sink;
pub mod stt;
pub mod token;
pub mod types;
pub mod wav;

pub use config::{
    ChannelConfig, CopilotConfig, EchoAction, EchoGuardConfig, EngineConfig, SinkConfig,
    SttBackend, SttConfig,
};
pub use engine::{CopilotDeps, CopilotEngine};
pub use error::CopilotError;
pub use handle::{CopilotEventListener, CopilotHandle, EventStream, StartParams};
pub use sink::{HttpSegmentSink, MemorySink, SegmentSink};
pub use stt::{EphemeralRealtimeStt, FakeStt};
pub use token::{HttpSttTokenProvider, SttToken, SttTokenProvider, TokenManager};
pub use types::{
    Channel, ChannelStats, CopilotEvent, EngineStats, PushStatus, SegmentRecord, SinkStats,
    StatusKind,
};
