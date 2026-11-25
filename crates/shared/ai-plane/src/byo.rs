//! Per-workspace provider resolution: cache, fallback policy and Redis invalidation
//! (docs/architecture/byo-llm.md sections 6.3 and 9).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures::StreamExt;
use secrecy::ExposeSecret;
use tokio::task::JoinHandle;
use warptalk_assistant::application::ports::{LlmConfigRepository, SecretVault};
use warptalk_assistant::domain::byo::{
    AI_CONFIG_CHANNEL, ConfigChanged, ConfigStatus, FallbackPolicy, LlmConfig, LlmProviderKind,
    LlmPurpose,
};
use warptalk_assistant::suggestion::{DataBoundary, KeySource, ResolveError, ResolvedLlm};
use warptalk_kernel::WorkspaceId;
use warptalk_llm::{ProviderConfig, ProviderId, ProviderRegistry};

use crate::policy::ExternalLlmPolicy;

/// Cache lifetime of a resolution; the pub/sub invalidation is the fast path, this the safety
/// net when a message is lost.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);

/// What the BYO layer needs from the outside.
#[derive(Clone)]
pub struct ByoDeps {
    pub repo: Arc<dyn LlmConfigRepository>,
    pub vault: Arc<dyn SecretVault>,
    pub policy: Arc<dyn ExternalLlmPolicy>,
    pub cache_ttl: Duration,
}

impl ByoDeps {
    pub fn new(
        repo: Arc<dyn LlmConfigRepository>,
        vault: Arc<dyn SecretVault>,
        policy: Arc<dyn ExternalLlmPolicy>,
    ) -> Self {
        Self {
            repo,
            vault,
            policy,
            cache_ttl: DEFAULT_CACHE_TTL,
        }
    }
}

/// BYO wiring from the environment: Postgres-backed configs, `AI_KEYS_KEK` vault (BYO off when
/// missing, with a clear error) and the workspace `externalLlmAllowed` consent flag.
pub fn byo_from_env(pool: sqlx::PgPool) -> ByoDeps {
    ByoDeps::new(
        Arc::new(warptalk_assistant::infrastructure::PgLlmConfigRepository::new(pool.clone())),
        crate::vault::vault_from_env(),
        Arc::new(crate::policy::PgExternalLlmPolicy::new(pool)),
    )
}

/// Result of looking at a workspace's override for one purpose.
#[derive(Clone)]
pub(crate) enum Outcome {
    /// No enabled override: use the platform default.
    NoOverride,
    Byo(ResolvedLlm),
    /// The workspace chose its own provider but it cannot serve. `fallback` is true only when
    /// the workspace enabled platform fallback and `external_llm_allowed` holds.
    Unavailable {
        reason: String,
        fallback: bool,
    },
}

struct Entry {
    outcome: Outcome,
    /// Config version the outcome was built from (cache key is `(workspace, version)`).
    version: Option<i64>,
    external_allowed: bool,
    loaded: Instant,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<(WorkspaceId, LlmPurpose), Entry>,
    /// Bumped on every invalidation so a load racing with one does not store stale data.
    epochs: HashMap<WorkspaceId, u64>,
    global_epoch: u64,
}

pub(crate) struct ByoResolver {
    deps: ByoDeps,
    inner: Mutex<Inner>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn provider_id(k: LlmProviderKind) -> ProviderId {
    match k {
        LlmProviderKind::OpenAi => ProviderId::OpenAi,
        LlmProviderKind::Anthropic => ProviderId::Anthropic,
        LlmProviderKind::Gemini => ProviderId::Gemini,
        LlmProviderKind::OpenAiCompatible => ProviderId::OpenAiCompatible,
    }
}

impl ByoResolver {
    pub(crate) fn new(deps: ByoDeps) -> Self {
        Self {
            deps,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub(crate) fn invalidate_workspace(&self, ws: WorkspaceId) {
        let mut g = lock(&self.inner);
        *g.epochs.entry(ws).or_insert(0) += 1;
        g.entries.retain(|(w, _), _| *w != ws);
    }

    pub(crate) fn invalidate_all(&self) {
        let mut g = lock(&self.inner);
        g.global_epoch += 1;
        g.entries.clear();
    }

    pub(crate) async fn resolve(
        &self,
        ws: WorkspaceId,
        purpose: LlmPurpose,
    ) -> Result<Outcome, ResolveError> {
        let (epoch, previous) = {
            let g = lock(&self.inner);
            if let Some(e) = g.entries.get(&(ws, purpose))
                && e.loaded.elapsed() < self.deps.cache_ttl
            {
                return Ok(e.outcome.clone());
            }
            let epoch = (g.global_epoch, g.epochs.get(&ws).copied().unwrap_or(0));
            let previous = g
                .entries
                .get(&(ws, purpose))
                .map(|e| (e.version, e.external_allowed, e.outcome.clone()));
            (epoch, previous)
        };

        let cfg = self
            .deps
            .repo
            .get_config(ws, purpose)
            .await
            .map_err(|e| ResolveError::Failed(format!("loading workspace AI config: {e}")))?;
        let external_allowed = self.deps.policy.external_llm_allowed(ws).await;
        let version = cfg.as_ref().map(|c| c.version);

        // Same config version and consent as the cached one: keep the built provider.
        let reuse = previous.and_then(|(v, allowed, outcome)| {
            (v == version && allowed == external_allowed && matches!(outcome, Outcome::Byo(_)))
                .then_some(outcome)
        });
        let outcome = match (reuse, cfg) {
            (Some(o), _) => o,
            (None, None) => Outcome::NoOverride,
            (None, Some(c)) => self.build(ws, &c, external_allowed).await?,
        };

        let mut g = lock(&self.inner);
        let current = (g.global_epoch, g.epochs.get(&ws).copied().unwrap_or(0));
        if current == epoch {
            g.entries.insert(
                (ws, purpose),
                Entry {
                    outcome: outcome.clone(),
                    version,
                    external_allowed,
                    loaded: Instant::now(),
                },
            );
        }
        Ok(outcome)
    }

    async fn build(
        &self,
        ws: WorkspaceId,
        cfg: &LlmConfig,
        external_allowed: bool,
    ) -> Result<Outcome, ResolveError> {
        if !cfg.enabled {
            return Ok(Outcome::NoOverride);
        }
        let fallback = cfg.fallback_policy == FallbackPolicy::PlatformIfAllowed && external_allowed;
        let unavailable = |reason: &str| {
            Ok(Outcome::Unavailable {
                reason: format!("workspace AI provider unavailable: {reason}"),
                fallback,
            })
        };
        if cfg.status == ConfigStatus::Invalid {
            return unavailable(
                cfg.last_error_code
                    .as_deref()
                    .unwrap_or("configuration invalid"),
            );
        }
        if cfg.data_boundary == warptalk_assistant::domain::byo::BoundaryKind::External
            && !external_allowed
        {
            // BYO is still a third-party endpoint: consent applies, and never falls back to
            // the (also external) platform provider.
            return Ok(Outcome::Unavailable {
                reason: "external LLM use is not allowed for this workspace".into(),
                fallback: false,
            });
        }
        if !self.deps.vault.is_enabled() {
            return unavailable("BYO credentials are disabled on this deployment");
        }
        let stored = self
            .deps
            .repo
            .get_credential(ws, cfg.provider)
            .await
            .map_err(|e| ResolveError::Failed(format!("loading workspace credential: {e}")))?;
        let Some(stored) = stored else {
            return unavailable("no credential stored");
        };
        let key = match self.deps.vault.open(ws, cfg.provider, &stored.sealed) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(workspace_id = %ws, provider = cfg.provider.as_str(), error = %e, "cannot decrypt workspace credential");
                return unavailable("credential cannot be decrypted");
            }
        };
        let mut pc = ProviderConfig::new(provider_id(cfg.provider), key.expose_secret());
        pc.base_url = cfg.base_url.clone();
        if let Some(ms) = cfg.timeout_ms.and_then(|v| u64::try_from(v).ok()) {
            pc.timeout_ms = ms;
        }
        match ProviderRegistry::build(pc) {
            Ok(provider) => Ok(Outcome::Byo(ResolvedLlm {
                provider,
                model: Some(cfg.model.clone()).filter(|m| !m.trim().is_empty()),
                boundary: DataBoundary::from(cfg.data_boundary),
                // A workspace credential is BYO whatever endpoint it points at.
                key_source: KeySource::Workspace,
            })),
            Err(e) => {
                tracing::warn!(workspace_id = %ws, provider = cfg.provider.as_str(), error = %e, "cannot build workspace provider");
                unavailable("provider configuration rejected")
            }
        }
    }

    /// Follow `AI_CONFIG_CHANNEL`; on (re)connect everything is dropped since messages may have
    /// been missed.
    pub(crate) fn spawn_listener(self: &Arc<Self>, redis_url: &str) -> JoinHandle<()> {
        let me = self.clone();
        let url = redis_url.to_owned();
        tokio::spawn(async move {
            loop {
                match warptalk_bus::subscribe(&url, &[AI_CONFIG_CHANNEL.to_owned()]).await {
                    Ok(stream) => {
                        me.invalidate_all();
                        let mut stream = Box::pin(stream);
                        while let Some((_, payload)) = stream.next().await {
                            match serde_json::from_str::<ConfigChanged>(&payload) {
                                Ok(ev) => {
                                    me.invalidate_workspace(WorkspaceId::from_uuid(ev.workspace_id))
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "bad ai-config change event");
                                    me.invalidate_all();
                                }
                            }
                        }
                        tracing::warn!("ai-config invalidation stream ended; reconnecting");
                    }
                    Err(e) => tracing::warn!(error = %e, "ai-config invalidation subscribe failed"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }
}
