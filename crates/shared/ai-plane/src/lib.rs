//! AI provider resolution shared by api and worker: platform keys + per-workspace BYO credentials
//! (docs/architecture/byo-llm.md).
//!
//! - [`ai`]: [`ai::AiProviders`], the resolver (`workspace override -> platform default`).
//! - [`byo`]: per-workspace resolution cache, fallback policy, Redis invalidation.
//! - [`vault`]: envelope encryption of workspace API keys ([`vault::KeyVault`]).
//! - [`validator`]: key validation against the provider with an SSRF guard.
//! - [`knowledge`]: vector store selection and the suggestion retriever.
//!
//! This crate sits above the assistant and knowledge modules on purpose: it implements their
//! ports (`LlmResolver`, `KeyValidator`, `SecretVault`, `ConfigNotifier`, `KnowledgeRetriever`)
//! so modules never depend on each other.

pub mod ai;
pub mod byo;
pub mod knowledge;
pub mod notifier;
pub mod policy;
pub mod validator;
pub mod vault;

pub use byo::{ByoDeps, byo_from_env};
pub use notifier::RedisConfigNotifier;
pub use policy::{ExternalLlmPolicy, PgExternalLlmPolicy, StaticExternalPolicy};
pub use validator::{BaseUrlPolicy, LlmKeyValidator};
pub use vault::{DisabledVault, KeyVault, vault_from_env};
