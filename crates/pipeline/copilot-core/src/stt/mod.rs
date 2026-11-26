//! STT backends behind `warptalk_pipeline_runtime::SpeechToText`.

mod fake;
mod realtime;
mod server;

pub use fake::FakeStt;
pub use realtime::EphemeralRealtimeStt;
pub use server::{NoticeBoard, ServerAudioStt, TimingBoard};
