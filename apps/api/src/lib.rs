//! Business plane composition root: wires modules into one axum router.

pub mod settings_http;
pub mod wiring;

pub use wiring::{
    ApiOptions, IdentityConsentDirectory, IdentityUserDirectory, LiveKitSettings,
    build as build_router,
};
