//! Post-session summary, minutes and action items (ADR 0001 item 7; spec 10 sections 3.9-3.11).
//!
//! The worker consumes `stt:results` in group `meeting-summary`; the `__MEETING_END__` marker
//! enqueues one job per room and transcript version, an explicit regenerate (REST) adds the
//! next version. A job reads the final transcript through [`ports::TranscriptSource`], summarizes
//! it (map-reduce over windows for long meetings) with the `Purpose::Summary` provider and stores
//! the validated result in `assistant.meeting_summaries` / `assistant.action_items`.
//! Tunables live in the `summary` settings namespace ([`config::SummaryConfig`]).

pub mod config;
pub mod model;
pub mod pg_access;
pub mod pg_sources;
pub mod ports;
pub mod prompt;
pub mod redis_ports;
pub mod repo;
pub mod service;
pub mod worker;

pub use config::{NS_SUMMARY, SummaryConfig, register as register_config};
pub use model::{
    ActionItem, ActionItemDraft, SUMMARY_GROUP, SummaryContent, SummaryJob, SummaryMeta,
    SummaryRequest, SummaryResultEvent, SummaryStatus,
};
pub use pg_access::{PgExternalLlmGate, PgRoomAccess};
pub use pg_sources::{PgRoomMetaSource, PgTranscriptSource};
pub use ports::{
    AllowAllExternalLlm, ExternalLlmGate, FollowUpSource, LiveFollowUp, NoBus, NoFollowUps,
    RoomAccess, RoomAccessInfo, RoomMeta, RoomMetaSource, SummaryBus, TranscriptData,
    TranscriptSegment, TranscriptSource, WorkspaceScope,
};
pub use redis_ports::{RedisFollowUpSource, RedisSummaryBus};
pub use repo::PgSummaryRepository;
pub use service::{RunOutcome, SummaryDeps, SummaryService};
pub use worker::SummaryWorker;
