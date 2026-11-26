//! Cross-meeting memory: what the host left open in earlier meetings of the workspace.
//!
//! [`MeetingMemory`] is the public port other features consume (summary generation today; the
//! chat and suggestion agents later): given the current room it returns a [`PriorContext`] with
//! the host's still-open action items and the last meeting summaries. [`PgMeetingMemory`] reads
//! only assistant's own tables (`meeting_summaries`, `action_items`); room facts (host, series,
//! title) are stamped on the summary row when its job runs, so no cross-schema join is needed.
//! Limits come from the `summary` settings namespace (`memory_*` fields of
//! [`crate::summary::SummaryConfig`]); callers pass them in [`PriorQuery`].

pub mod ports;
pub mod prompt;

pub use ports::{
    MeetingMemory, NoMemory, OpenItem, PgMeetingMemory, PriorContext, PriorQuery, PriorSummary,
};
