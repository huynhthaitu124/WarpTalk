//! Redis key builders.

use warptalk_kernel::{RoomId, WorkspaceId};

// spec: 10 §3.7 (TTL 12 h)
pub fn audio_routes(room: RoomId) -> String {
    format!("translationRoom:{room}:audio_routes")
}
// spec: 08 §2 (per-room flash mode override)
pub fn flash_mode(room: RoomId) -> String {
    format!("translationRoom:{room}:flash_mode")
}
// spec: 10 §3.7
pub fn clone_intent(room: RoomId) -> String {
    format!("translationRoom:{room}:clone_intent")
}
// spec: 10 §5.4
pub fn ai_service_suspended(room: RoomId) -> String {
    format!("translationRoom:{room}:ai_service_suspended")
}
// spec: 10 §5.4
pub fn ai_service_state(room: RoomId) -> String {
    format!("translationRoom:{room}:ai_service_state")
}
// spec: 10 §5.4
pub fn workspace_ai_service_suspended(ws: WorkspaceId) -> String {
    format!("workspace:{ws}:ai_service_suspended")
}
// spec: 10 §5.4
pub fn workspace_ai_service_state(ws: WorkspaceId) -> String {
    format!("workspace:{ws}:ai_service_state")
}
// spec: 12 §5 (set of paused segment ids)
pub fn transcript_paused_segments(room: RoomId) -> String {
    format!("translationRoom:{room}:transcript_paused_segments")
}
// spec: 10 §6
pub fn idle_since(room: RoomId) -> String {
    format!("translationRoom:{room}:idle_since")
}
// spec: 11 §6.3 (24 h cache)
pub fn meeting_room_cache(room: RoomId) -> String {
    format!("meeting:room:v2:{room}")
}
/// Rust design: which pipeline instance owns a room.
pub fn pipeline_room_owner(room: RoomId) -> String {
    format!("pipeline:room:{room}:owner")
}
/// spec: 12 §6.1
pub const TRANSCRIPT_CONSUMER_METRICS: &str = "transcript:persistence:consumer:metrics";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn builders() {
        let r = RoomId::new();
        let w = WorkspaceId::new();
        assert_eq!(audio_routes(r), format!("translationRoom:{r}:audio_routes"));
        assert_eq!(flash_mode(r), format!("translationRoom:{r}:flash_mode"));
        assert_eq!(clone_intent(r), format!("translationRoom:{r}:clone_intent"));
        assert_eq!(
            ai_service_suspended(r),
            format!("translationRoom:{r}:ai_service_suspended")
        );
        assert_eq!(
            workspace_ai_service_suspended(w),
            format!("workspace:{w}:ai_service_suspended")
        );
        assert_eq!(
            transcript_paused_segments(r),
            format!("translationRoom:{r}:transcript_paused_segments")
        );
        assert_eq!(idle_since(r), format!("translationRoom:{r}:idle_since"));
        assert_eq!(pipeline_room_owner(r), format!("pipeline:room:{r}:owner"));
        assert_eq!(meeting_room_cache(r), format!("meeting:room:v2:{r}"));
    }
}
