//! Latency-critical front of the realtime pipeline: VAD, hysteresis, adaptive-hangover
//! endpointing, chunk cap, energy and per-speaker level gates.
//!
//! Pure and synchronous (no tokio, no IO) so it can be unit-tested and benchmarked. Audio is
//! mono `f32` at 16 kHz, fed in 32 ms frames (512 samples).
//!
//! NOTE: this crate deliberately does not depend on `warptalk-audio` yet: that crate is not in
//! `[workspace.dependencies]` (root `Cargo.toml` is off limits to this task), so the few helpers
//! needed (rms, pre-speech ring) are private here. Swap them for the shared ones once it is.

mod config;
mod endpointer;
mod floor;
mod vad;

pub use config::EndpointerConfig;
pub use endpointer::{
    DropReason, EndReason, EndpointEvent, Endpointer, FRAME_MS, FRAME_SAMPLES, SAMPLE_RATE_HZ,
};
pub use floor::{FloorVerdict, SpeakerLevelFloor};
pub use vad::{EnergyVad, Vad};

// TODO(near_field_gate, far_side_gate): both are off by default upstream (recall over precision;
// the near-field gate dropped valid middle chunks in production). Not ported.
// TODO(silero): `Vad` implementation backed by Silero via ONNX Runtime, in a later change.
