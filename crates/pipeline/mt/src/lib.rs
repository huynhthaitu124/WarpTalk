//! Machine translation stage: STT segments in, per-language [`Translation`]s out.
//!
//! See [`stage`] for the latency design decisions. Building blocks:
//! - [`Translator`] port; [`OpenAiTranslator`] = OpenAI Realtime text session ([`realtime`], warm
//!   socket pool) for the first sentence with a chat-model fallback through
//!   [`warptalk_llm::LlmProvider`] (batch with JSON-schema output for the rest);
//! - [`glossary`]: workspace > global precedence, relevance selection, ASR-repair evidence;
//! - [`speculative`]: prefetch cache keyed by normalised text with TTL;
//! - [`order`]: bounded per-speaker publish ordering;
//! - [`MtConfig`]: all knobs, `Deserialize` with defaults, `validate`, hot-reloadable via `watch`.
//!
//! Config namespace: store `MtConfig` as JSON under the `pipeline` settings namespace, key `mt`.

mod config;
mod error;
pub mod glossary;
pub mod lang;
pub mod order;
mod polish;
mod ports;
pub mod prompt;
pub mod realtime;
pub mod speculative;
mod stage;
pub mod testing;
pub mod text;
mod translator;
mod types;

pub use config::{ChatConfig, MtConfig, PolishConfig, RealtimeConfig, SpeculativeConfig};
pub use error::MtError;
pub use glossary::{
    GlossaryProvider, GlossaryTerm, InMemoryGlossary, MatchKind, SelectedTerm, merge_glossaries,
    select_terms,
};
pub use polish::{choose_transcript, is_faithful};
pub use ports::{
    ContextProvider, NeverSkip, NoContext, SkipPolicy, StaticTargets, TargetLanguages,
};
pub use prompt::{BatchRequest, TranslateRequest};
pub use realtime::{RealtimeCall, RealtimeClient, RealtimeReply};
pub use stage::{
    MtDeps, MtOutputs, STAGE_MT_DONE, STAGE_MT_FIRST_TOKEN, STAGE_MT_ORDER_WAIT, StageHandle,
    TranslationStage,
};
pub use translator::{BatchOutput, OpenAiTranslator, TranslateOutput, Translator};
pub use types::{MtEvent, MtStamps, SkipReason, SpeculativeHint, Translation};
