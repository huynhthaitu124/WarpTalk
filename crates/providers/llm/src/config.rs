//! Provider configuration and registry.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use secrecy::SecretString;
use serde::{Deserialize, Deserializer, Serialize};

use crate::anthropic::AnthropicProvider;
use crate::error::LlmError;
use crate::gemini::GeminiProvider;
use crate::model::ProviderId;
use crate::openai::OpenAiProvider;
use crate::openai_compatible::OpenAiCompatibleProvider;
use crate::provider::{EmbeddingProvider, LlmProvider};
use crate::retry::{RetryPolicy, Timeout};

fn default_timeout_ms() -> u64 {
    60_000
}
fn default_max_retries() -> u32 {
    2
}
fn de_secret<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
    Ok(SecretString::from(String::deserialize(d)?))
}

/// Already-decrypted per-workspace provider config. The key is never serialized or printed.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderId,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(skip_serializing, deserialize_with = "de_secret")]
    pub api_key: SecretString,
    #[serde(default)]
    pub default_headers: BTreeMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

impl ProviderConfig {
    pub fn new(kind: ProviderId, api_key: impl Into<String>) -> Self {
        Self {
            kind,
            base_url: None,
            api_key: SecretString::from(api_key.into()),
            default_headers: BTreeMap::new(),
            timeout_ms: default_timeout_ms(),
            max_retries: default_max_retries(),
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            // Header values may carry credentials (e.g. Azure `api-key`): names only.
            .field(
                "default_headers",
                &self.default_headers.keys().collect::<Vec<_>>(),
            )
            .field("timeout_ms", &self.timeout_ms)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

pub struct ProviderRegistry;

impl ProviderRegistry {
    /// Builds the adapter wrapped as `Retry(Timeout(adapter))`.
    pub fn build(cfg: ProviderConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
        let inner: Arc<dyn LlmProvider> = match cfg.kind {
            ProviderId::OpenAi => Arc::new(OpenAiProvider::new(&cfg)?),
            ProviderId::Anthropic => Arc::new(AnthropicProvider::new(&cfg)?),
            ProviderId::Gemini => Arc::new(GeminiProvider::new(&cfg)?),
            ProviderId::OpenAiCompatible => Arc::new(OpenAiCompatibleProvider::new(&cfg)?),
        };
        let timeout = Duration::from_millis(cfg.timeout_ms);
        let inner = Timeout::wrap(timeout, inner);
        if cfg.max_retries == 0 {
            return Ok(inner);
        }
        let policy = RetryPolicy {
            max_retries: cfg.max_retries,
            total_deadline: timeout.saturating_mul(cfg.max_retries.saturating_add(1)),
            ..RetryPolicy::default()
        };
        Ok(policy.wrap(inner))
    }

    /// Embeddings are supported for OpenAI and Gemini only.
    pub fn build_embedding(cfg: ProviderConfig) -> Result<Arc<dyn EmbeddingProvider>, LlmError> {
        match cfg.kind {
            ProviderId::OpenAi => Ok(Arc::new(OpenAiProvider::new(&cfg)?)),
            ProviderId::Gemini => Ok(Arc::new(GeminiProvider::new(&cfg)?)),
            other => Err(LlmError::InvalidRequest(format!(
                "provider {other} has no embedding support"
            ))),
        }
    }
}
