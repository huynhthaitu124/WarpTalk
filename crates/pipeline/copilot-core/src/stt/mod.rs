//! STT backends behind `warptalk_pipeline_runtime::SpeechToText`.

mod fake;
mod realtime;

pub use fake::FakeStt;
pub use realtime::EphemeralRealtimeStt;
