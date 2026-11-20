//! Segments, corrections, translation contents, dubbing records, glossaries, exports, finalize (schema transcript).
//!
//! Layers (dependencies point inward):
//! - `domain`: entities, value objects, state machines, domain errors. No IO.
//! - `application`: use cases and ports (traits) this module needs from the outside.
//! - `infrastructure`: adapters implementing the ports (sqlx, Redis, providers).
//! - `http`: axum handlers and request/response DTOs.

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;
