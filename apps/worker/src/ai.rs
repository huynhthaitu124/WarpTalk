//! Platform AI provider resolution shared by the worker and the API.
//!
//! Providers are built from the platform settings namespace `ai_providers` (which provider and
//! model serves each capability) and platform API keys from the environment
//! (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, optional `*_BASE_URL`). Built
//! providers are cached per `(kind, base_url)`; because the settings are read on every
//! resolution, a settings change takes effect on the next call without a restart.
//!
//! TODO(byo-llm): per-workspace BYO keys (docs/architecture/byo-llm.md) plug in at
//! [`AiProviders::llm_for`]: look up the workspace's encrypted provider config first, mark it
//! [`DataBoundary::WorkspaceControlled`] when it points into the tenant, and fall back to the
//! platform keys below.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;
use warptalk_assistant::suggestion::{DataBoundary, LlmResolver, Purpose, ResolveError, ResolvedLlm};
use warptalk_kernel::WorkspaceId;
use warptalk_llm::{
    Capabilities, EmbeddingProvider, Embeddings, LlmError, LlmProvider, ProviderConfig,
    ProviderId, ProviderRegistry, Request, Response, StreamEvent,
};
use warptalk_settings::namespaces::AiProvidersSettings;
use warptalk_settings::{NS_AI_PROVIDERS, ScopeChain, SettingsStore};

type CacheKey = (ProviderId, Option<String>);

/// One platform credential.
#[derive(Clone)]
pub struct PlatformKey {
    pub api_key: String,
    pub base_url: Option<String>,
}

/// Platform API keys per provider kind.
#[derive(Clone, Default)]
pub struct PlatformKeys {
    keys: HashMap<ProviderId, PlatformKey>,
}

impl std::fmt::Debug for PlatformKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print keys.
        f.debug_list().entries(self.keys.keys()).finish()
    }
}

impl PlatformKeys {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let mut out = Self::default();
        for (kind, key_var, url_var) in [
            (ProviderId::OpenAi, "OPENAI_API_KEY", "OPENAI_BASE_URL"),
            (ProviderId::Anthropic, "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"),
            (ProviderId::Gemini, "GEMINI_API_KEY", "GEMINI_BASE_URL"),
        ] {
            if let Some(api_key) = get(key_var) {
                out.keys.insert(
                    kind,
                    PlatformKey {
                        api_key,
                        base_url: get(url_var),
                    },
                );
            }
        }
        out
    }

    pub fn with(mut self, kind: ProviderId, key: PlatformKey) -> Self {
        self.keys.insert(kind, key);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn has(&self, kind: ProviderId) -> bool {
        self.keys.contains_key(&kind)
    }
}

fn parse_kind(name: &str) -> Option<ProviderId> {
    match name.trim().to_ascii_lowercase().as_str() {
        "openai" => Some(ProviderId::OpenAi),
        "anthropic" => Some(ProviderId::Anthropic),
        "gemini" | "google" => Some(ProviderId::Gemini),
        _ => None,
    }
}

pub struct AiProviders {
    settings: SettingsStore,
    keys: PlatformKeys,
    llm: Mutex<HashMap<CacheKey, Arc<dyn LlmProvider>>>,
    embed: Mutex<HashMap<CacheKey, Arc<dyn EmbeddingProvider>>>,
}

impl AiProviders {
    pub fn new(settings: SettingsStore, keys: PlatformKeys) -> Arc<Self> {
        Arc::new(Self {
            settings,
            keys,
            llm: Mutex::new(HashMap::new()),
            embed: Mutex::new(HashMap::new()),
        })
    }

    pub fn from_env(settings: SettingsStore) -> Arc<Self> {
        Self::new(settings, PlatformKeys::from_env())
    }

    /// Whether any platform key is configured.
    pub fn any_key(&self) -> bool {
        !self.keys.is_empty()
    }

    async fn choices(&self) -> AiProvidersSettings {
        self.settings
            .get::<AiProvidersSettings>(NS_AI_PROVIDERS, ScopeChain::platform())
            .await
            .unwrap_or_default()
    }

    fn config(&self, kind: ProviderId) -> Option<ProviderConfig> {
        let k = self.keys.keys.get(&kind)?;
        let mut cfg = ProviderConfig::new(kind, k.api_key.clone());
        cfg.base_url = k.base_url.clone();
        Some(cfg)
    }

    fn llm_provider(&self, kind: ProviderId) -> Result<Arc<dyn LlmProvider>, ResolveError> {
        let cfg = self.config(kind).ok_or(ResolveError::NotConfigured)?;
        let key = (kind, cfg.base_url.clone());
        let mut cache = self
            .llm
            .lock()
            .map_err(|_| ResolveError::Failed("provider cache poisoned".into()))?;
        if let Some(p) = cache.get(&key) {
            return Ok(p.clone());
        }
        let p = ProviderRegistry::build(cfg).map_err(|e| ResolveError::Failed(e.to_string()))?;
        cache.insert(key, p.clone());
        Ok(p)
    }

    fn embedding_provider(&self, kind: ProviderId) -> Result<Arc<dyn EmbeddingProvider>, LlmError> {
        let cfg = self
            .config(kind)
            .ok_or_else(|| LlmError::Auth(format!("no platform key for provider {kind}")))?;
        let key = (kind, cfg.base_url.clone());
        let mut cache = self
            .embed
            .lock()
            .map_err(|_| LlmError::InvalidRequest("provider cache poisoned".into()))?;
        if let Some(p) = cache.get(&key) {
            return Ok(p.clone());
        }
        let p = ProviderRegistry::build_embedding(cfg)?;
        cache.insert(key, p.clone());
        Ok(p)
    }

    /// Provider and model for an LLM purpose (platform keys only; BYO hook above).
    pub async fn llm_for(
        &self,
        _workspace: WorkspaceId,
        _purpose: Purpose,
    ) -> Result<ResolvedLlm, ResolveError> {
        let choice = self.choices().await.llm;
        let kind = parse_kind(&choice.provider).ok_or(ResolveError::NotConfigured)?;
        let provider = self.llm_provider(kind)?;
        Ok(ResolvedLlm {
            provider,
            model: Some(choice.model).filter(|m| !m.trim().is_empty()),
            // Platform keys reach third-party endpoints.
            boundary: DataBoundary::External,
        })
    }

    /// Provider name recorded in the knowledge embedding profile.
    pub async fn embedding_provider_name(&self) -> String {
        self.choices().await.embedding.provider
    }

    /// Embedder that resolves the configured provider on every call. Without a platform key it
    /// logs and fails with a non-retryable error, so indexing jobs are dropped, not retried.
    pub fn embedder(self: &Arc<Self>) -> Arc<dyn EmbeddingProvider> {
        Arc::new(ResolvingEmbedder(self.clone()))
    }

    /// Chat provider that resolves the configured provider on every call (re-rank, facts).
    /// `None` when no platform key is configured at all.
    pub fn chat(self: &Arc<Self>) -> Option<Arc<dyn LlmProvider>> {
        self.any_key()
            .then(|| Arc::new(ResolvingLlm(self.clone())) as Arc<dyn LlmProvider>)
    }
}

#[async_trait]
impl LlmResolver for AiProviders {
    async fn for_purpose(
        &self,
        workspace: WorkspaceId,
        purpose: Purpose,
    ) -> Result<ResolvedLlm, ResolveError> {
        self.llm_for(workspace, purpose).await
    }
}

struct ResolvingEmbedder(Arc<AiProviders>);

#[async_trait]
impl EmbeddingProvider for ResolvingEmbedder {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        let choice = self.0.choices().await.embedding;
        let kind = parse_kind(&choice.provider).ok_or_else(|| {
            LlmError::InvalidRequest(format!("unsupported embedding provider {}", choice.provider))
        })?;
        match self.0.embedding_provider(kind) {
            Ok(p) => p.embed(model, inputs).await,
            Err(e) => {
                tracing::warn!(provider = %kind, error = %e, "embeddings disabled; skipping");
                Err(e)
            }
        }
    }
}

struct ResolvingLlm(Arc<AiProviders>);

impl ResolvingLlm {
    async fn current(&self) -> Result<Arc<dyn LlmProvider>, LlmError> {
        let choice = self.0.choices().await.llm;
        let kind = parse_kind(&choice.provider).ok_or_else(|| {
            LlmError::InvalidRequest(format!("unsupported llm provider {}", choice.provider))
        })?;
        self.0
            .llm_provider(kind)
            .map_err(|e| LlmError::Auth(e.to_string()))
    }
}

#[async_trait]
impl LlmProvider for ResolvingLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities::conservative()
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        self.current().await?.complete(req).await
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        self.current().await?.stream(req).await
    }
    async fn validate(&self) -> Result<(), LlmError> {
        self.current().await?.validate().await
    }
}
