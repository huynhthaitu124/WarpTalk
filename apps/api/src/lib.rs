//! Business plane composition root: wires modules into one axum router.

pub mod auth_wiring;
pub mod billing_wiring;
pub mod features_http;
pub mod room_effects;
pub mod settings_http;
pub mod suggestions_http;
pub mod wiring;

pub use wiring::{
    ApiOptions, IdentityConsentDirectory, IdentityUserDirectory, LiveKitSettings,
    build as build_router,
};
