//! LiveKit: access tokens, room service (Twirp), webhook verification.
//!
//! Hand-rolled on purpose (no livekit crates): HS256 JWTs with the LiveKit `video` grant,
//! webhook signature + body-hash verification, and a small Twirp/JSON `RoomService` client.

mod config;
mod error;
mod room_service;
mod token;
mod webhook;

pub use config::LiveKitConfig;
pub use error::LiveKitError;
pub use room_service::{
    InMemoryRoomService, NoopRoomService, RoomService, RoomServiceCall, RoomServiceClient,
};
pub use token::{
    AccessTokenBuilder, TokenClaims, VideoGrant, admin_token, bridge_token, participant_token,
    pipeline_agent_token,
};
pub use webhook::{
    EgressInfo, FileResult, WebhookEvent, WebhookParticipant, WebhookRoom, WebhookTrack,
    sign_webhook, verify_webhook,
};
