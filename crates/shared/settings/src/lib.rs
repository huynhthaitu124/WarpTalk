//! Runtime configuration store: platform defaults + workspace/room overrides, hot-reloaded.
//!
//! Values are JSON documents per namespace, deep-merged `Platform < Workspace < Room`,
//! cached in-process (TTL + Redis-driven invalidation) and audited on every change.

pub mod namespaces;
mod scope;
mod store;
mod validate;

pub use namespaces::{
    AiProvidersSettings, FeatureFlags, NS_AI_PROVIDERS, NS_FEATURES, NS_PIPELINE, NS_RAG,
    NS_SUGGESTION, PipelineSettings, ProviderChoice, RagSettings, SuggestionCategory,
    SuggestionSettings, register_defaults,
};
pub use scope::{Scope, ScopeChain};
pub use store::{CHANNEL, ChangeEvent, DEFAULT_TTL, PutOutcome, SettingsStore};
pub use validate::{Validate, check_range, deep_merge};

use warptalk_db::{Migration, ModuleMigrations};

/// Creates `public.settings_entries` and `public.settings_audit`.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "settings",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};
