//! Cartesia TTS client: warm WebSocket pool, prosodic contexts, voice catalog, voice cloning.
//!
//! # Latency design decisions
//!
//! - **Warm pool** ([`WarmPool`]): `pool_size` sockets are dialled ahead of time; claiming one
//!   is instant, a background refill follows. The dial (p50 427.6 ms) is paid by nobody. Idle
//!   age and a non-blocking liveness probe are checked at claim, so a dead socket is never
//!   handed to a spoken sentence.
//! - **Streaming**: audio is forwarded chunk by chunk as the vendor emits it
//!   ([`SpeakContext::speak`]), never buffered per sentence.
//! - **Contexts** give prosody continuity between consecutive sentences of one speaker; the
//!   caller keeps one open only while the next sentence is already queued, and closes it with
//!   [`SpeakContext::close`] otherwise, because an idle context holds a vendor slot.
//! - **Bounded everything**: connect, per-sentence and close-ack timeouts; a hang becomes an
//!   error the caller can degrade from.
//!
//! The wire protocol follows the upstream integration; every detail the vendor docs could not
//! be checked for here is marked `UNVERIFIED` ([`protocol`], [`http`]).

mod client;
mod config;
mod context;
mod error;
pub mod http;
mod pool;
pub mod protocol;

pub use client::{CartesiaClient, default_voice_id};
pub use config::CartesiaConfig;
pub use context::{SpeakContext, SpeakStats};
pub use error::CartesiaError;
pub use http::{CartesiaHttp, VoiceCatalog, VoiceInfo, base_language};
pub use pool::{PoolStats, WarmPool, WarmSocket, WsStream, connect};
pub use protocol::GenerationConfig;
