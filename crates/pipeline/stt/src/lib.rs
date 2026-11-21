//! OpenAI Realtime transcription over WebSocket, as a `warptalk-pipeline-runtime`
//! [`SpeechToText`](warptalk_pipeline_runtime::SpeechToText).
//!
//! # Latency design decisions
//!
//! - **Warm pool** ([`WarmPool`]): `pool_size` (default 4) sockets are connected ahead of time;
//!   claiming one is instant and a background refill follows. The speaker's `session.update`
//!   is fire-and-forget in front of the audio (no round trip). Dead / over-age sockets are
//!   discarded at claim (non-blocking liveness probe), so a first speaker never pays for a
//!   closed connection.
//! - **Stream while speaking** (flash mode): live frames go out as base64 pcm16 @ 24 kHz as
//!   they arrive; at utterance end only `input_audio_buffer.commit` is sent and
//!   `SttSession::commit` returns without waiting for the transcript.
//! - **Early sentences at commit**: the streaming model has usually transcribed most of the
//!   turn before commit, so at the `committed` event every complete sentence in the buffered
//!   deltas is emitted at once (no wait for `completed`), the rest as deltas arrive. Output is
//!   append-only; `completed` contributes only the remainder (see [`turns`]).
//! - **Early sentence boundaries** are detected on the delta stream with a terminator-aware
//!   splitter ([`sentence`]) that flushes a clean `.`/`!`/`?` at once but holds back decimals
//!   and abbreviations, and handles `。！？` without whitespace.
//! - **No ordering waits**: turns are emitted in commit order from a single reader, so the
//!   runtime does not need the original's 10 s publish-order wait.
//! - **Resampling is per frame** (16 -> 24 kHz, linear): a one-sample interpolation edge per
//!   32 ms frame, inaudible to the recognizer; stateless so frames can be fed from any task.
//! - Not done: coalescing several frames per `append` (32 ms granularity costs ~31 small
//!   messages/s per speaker, which keeps the model fed as early as possible).
//!
//! Protocol types and the documentation URL: [`protocol`].

mod config;
pub mod far_side_dedupe;
mod pool;
pub mod protocol;
pub mod sentence;
mod session;
pub mod turns;

pub use config::{LogprobFilter, OpenAiSttConfig, base_language};
pub use pool::{PoolStats, WarmPool, WarmSocket, connect};
pub use session::{OpenAiRealtimeStt, encode_pcm16_24k};
