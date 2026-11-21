//! Business plane composition root: wires modules into one axum router.

// Shared with the worker without a crate dependency (`warptalk-worker` is not a workspace
// dependency and apps must not depend on apps): the same source files, compiled here too.
#[path = "../../worker/src/ai.rs"]
pub mod ai;
#[path = "../../worker/src/knowledge.rs"]
pub mod knowledge;
pub mod settings_http;
pub mod wiring;

pub use wiring::{ApiOptions, IdentityUserDirectory, LiveKitSettings, build as build_router};
