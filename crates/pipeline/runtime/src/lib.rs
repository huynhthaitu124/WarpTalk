//! Realtime pipeline runtime: message types with latency stamps, the STT stage contract, and
//! the per-speaker / per-room tokio tasks that connect ingress (VAD + endpointing) to STT.
//! No network here; transports live in `warptalk-stt` and friends (ADR-2: one process, stages
//! joined by bounded tokio channels instead of Redis hops).
//!
//! # Latency design decisions
//!
//! 1. **Audio is never back-pressured.** [`SpeakerHandle::push`] is synchronous and takes a
//!    short std mutex; a full queue evicts the *oldest* frame and bumps
//!    `warptalk_pipeline_frames_dropped_total`. A stalled STT socket costs stale audio, not
//!    seconds of added delay, and never blocks the LiveKit reader.
//! 2. **Flash mode streams live frames** (ungated, including pre-speech padding) to the STT
//!    session while the speaker talks, so at utterance end only `commit` remains (spec 08
//!    section 2.2). Closed-chunk mode is supported through the same trait.
//! 3. **`commit` only issues the commit.** The speaker worker goes straight back to
//!    endpointing/streaming the next turn; a separate relay task restores order and stamps
//!    segments, so a slow transcript of turn N does not delay turn N+1's audio.
//! 4. **Early sentences are forwarded the moment STT yields them**, before the turn is
//!    complete; the output channel is bounded and awaited only on the STT side (backpressure
//!    towards STT, never towards audio).
//! 5. **STT sessions are claimed eagerly** (`eager_session_open`, default on): the warm socket
//!    is taken when the speaker joins, off the critical path. The stage stamps `stt_commit`
//!    after the session is ready so connect time is not misattributed to STT.
//! 6. **Per-speaker order is structural**: one worker issues commits sequentially; the relay
//!    numbers segments in arrival order (`SttSegment::seq`). No ordering waits (the original's
//!    10 s publish-turn wait is gone).
//! 7. **Hot reload without clipping**: a config change is applied when the endpointer is idle
//!    (next utterance); the endpointer is rebuilt and the recent idle frames replayed so the
//!    pre-speech padding survives. Caveat: the per-speaker energy baseline restarts.
//! 8. Metrics (`warptalk_stage_latency_ms`): `endpoint_decision` (last speech frame -> end
//!    decision, i.e. the hangover as experienced), `stt_first_text` (commit -> first text,
//!    saturating at 0 when an early sentence beat the commit), `stt_final` (commit ->
//!    completed).
//!
//! Voice-activity detection runs inline on the speaker task: it is a few microseconds per
//! 32 ms frame for the energy VAD; a heavier model should expose a blocking-friendly `Vad`.

mod config;
mod error;
mod frame_queue;
mod messages;
mod room;
mod speaker;
mod stt;
pub mod testing;

pub use config::PipelineConfig;
pub use error::{PipelineError, SttError};
pub use frame_queue::{DROPPED_FRAMES_METRIC, FrameQueue, PushOutcome};
pub use messages::{AudioFrame, ParticipantId, RoomId, Stamps, SttSegment, Utterance};
pub use room::{RoomDeps, RoomPipeline};
pub use speaker::{
    STAGE_ENDPOINT_DECISION, STAGE_STT_FINAL, STAGE_STT_FIRST_TEXT, SpeakerHandle, SpeakerPipeline,
    SpeakerSpec, VadFactory,
};
pub use stt::{CommitRequest, SessionParams, SpeechToText, SttEvent, SttMode, SttSession};
