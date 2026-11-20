//! Business plane composition root: wires modules into one axum router.

pub mod wiring;

pub use wiring::{IdentityUserDirectory, build as build_router};
