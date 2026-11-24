//! Offline evaluation harness for the AI core (live suggestions and post-meeting summaries).
//!
//! A labelled synthetic meeting ([`dataset::Meeting`]) is replayed through the real
//! `SuggestionEngine` and the real `SummaryService`; the outputs are scored against the
//! meeting's expectations ([`scoring`]) and written as a [`report::Report`]. See
//! `docs/architecture/ai-eval.md`.

pub mod cli;
pub mod cost;
pub mod dataset;
pub mod fake;
pub mod harness;
pub mod judge;
pub mod providers;
pub mod recorder;
pub mod report;
pub mod scoring;
pub mod textmatch;
