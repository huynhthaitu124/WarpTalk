//! Shared kernel: ids, errors, paging, clock, tenancy primitives.

pub mod clock;
pub mod error;
pub mod ids;
pub mod paging;
pub mod tenancy;

pub use clock::{Clock, FixedClock, SystemClock};
pub use error::{AppError, AppResult};
pub use ids::{
    MeetingRoomId, MemberId, ParticipantId, RoomId, SegmentId, SessionId, TranscriptId, UserId,
    WorkspaceId,
};
pub use paging::{Page, PageRequest};
pub use tenancy::{Actor, WorkspaceRole};
