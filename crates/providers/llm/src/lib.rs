//! Provider-neutral LLM client (OpenAI, Anthropic, Gemini, OpenAI-compatible). See docs/architecture/byo-llm.md.
//!
//! Thin hand-written adapters over `reqwest`: each keeps cache/usage detail, opaque reasoning
//! blocks and spend-limit vs rate-limit classification that multi-provider crates flatten.

pub mod anthropic;
pub mod config;
pub mod error;
pub mod gemini;
mod http;
pub mod model;
pub mod openai;
pub mod openai_compatible;
pub mod provider;
pub mod retry;

pub use config::{ProviderConfig, ProviderRegistry};
pub use error::LlmError;
pub use model::*;
pub use provider::{Capabilities, EmbeddingProvider, LlmProvider, PromptCache};
pub use retry::{RetryPolicy, Timeout};
