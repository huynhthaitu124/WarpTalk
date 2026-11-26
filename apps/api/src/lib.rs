//! Business plane composition root: wires modules into one axum router.

pub mod assistant_chat;
pub mod room_effects;
pub mod settings_http;
pub mod suggestions_http;
pub mod wiring;

pub use wiring::{
    ApiOptions, IdentityConsentDirectory, IdentityUserDirectory, LiveKitSettings,
    build as build_router,
};
