//! Use cases behind `ai/workspaces/{id}/config` and `.../credentials/{provider}`.

use std::sync::Arc;
use std::time::Instant;

use secrecy::{ExposeSecret, SecretString};
use warptalk_kernel::{AppError, AppResult, Clock, UserId, WorkspaceId, WorkspaceRole};

use super::ports::{
    ConfigNotifier, KeyCheckError, KeyValidator, LlmConfigRepository, SecretVault, VaultError,
    WorkspaceAccess,
};
use crate::domain::byo::{
    BoundaryKind, CredentialMeta, LlmConfig, LlmConfigInput, LlmProviderKind, LlmPurpose,
    MAX_API_KEY_LEN, MIN_API_KEY_LEN, embedding_changed,
};

const BYO_DISABLED: &str = "BYO_DISABLED";

pub struct LlmConfigService {
    repo: Arc<dyn LlmConfigRepository>,
    vault: Arc<dyn SecretVault>,
    validator: Arc<dyn KeyValidator>,
    notifier: Arc<dyn ConfigNotifier>,
    access: Arc<dyn WorkspaceAccess>,
    clock: Arc<dyn Clock>,
}

#[derive(Debug)]
pub struct ConfigView {
    pub byo_enabled: bool,
    pub configs: Vec<LlmConfig>,
    pub credentials: Vec<CredentialMeta>,
}

/// `Some(input)` sets the purpose's override, `None` removes it (platform default again).
#[derive(Debug)]
pub struct PutConfigCommand {
    pub entries: Vec<(LlmPurpose, Option<LlmConfigInput>)>,
    /// Admin confirmed that `workspace_controlled` endpoints stay inside the workspace's tenant.
    pub acknowledge_workspace_controlled: bool,
}

#[derive(Debug)]
pub struct PutConfigResult {
    pub view: ConfigView,
    pub reindex_required: bool,
}

#[derive(Debug)]
pub struct TestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub error_code: Option<&'static str>,
}

fn vault_err(e: VaultError) -> AppError {
    match e {
        VaultError::Disabled(m) => AppError::coded(503, BYO_DISABLED, m),
        VaultError::Crypto(m) => {
            tracing::error!(error = %m, "credential crypto failure");
            AppError::internal("credential crypto failure")
        }
    }
}

/// Maps a provider check failure to the API error. Provider messages are not echoed: they could
/// quote the key back.
fn check_err(e: &KeyCheckError) -> AppError {
    match e {
        KeyCheckError::Rejected(_) => AppError::validation(
            "INVALID_API_KEY",
            "the provider rejected this API key or model",
        ),
        KeyCheckError::Account(_) => AppError::validation(
            "PROVIDER_ACCOUNT_PROBLEM",
            "the provider reports a quota or billing problem for this key",
        ),
        KeyCheckError::Unreachable(_) => AppError::coded(
            502,
            "PROVIDER_UNREACHABLE",
            "could not reach the provider to validate the key",
        ),
        KeyCheckError::BadBaseUrl(m) => AppError::validation("INVALID_BASE_URL", m.clone()),
    }
}

fn error_code(e: &KeyCheckError) -> &'static str {
    match e {
        KeyCheckError::Rejected(_) => "INVALID_API_KEY",
        KeyCheckError::Account(_) => "PROVIDER_ACCOUNT_PROBLEM",
        KeyCheckError::Unreachable(_) => "PROVIDER_UNREACHABLE",
        KeyCheckError::BadBaseUrl(_) => "INVALID_BASE_URL",
    }
}

fn clean_base_url(raw: Option<String>) -> AppResult<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.len() > 2048 || !(t.starts_with("https://") || t.starts_with("http://")) {
        return Err(AppError::validation(
            "INVALID_BASE_URL",
            "baseUrl must be an http(s) URL",
        ));
    }
    Ok(Some(t.to_owned()))
}

impl LlmConfigService {
    pub fn new(
        repo: Arc<dyn LlmConfigRepository>,
        vault: Arc<dyn SecretVault>,
        validator: Arc<dyn KeyValidator>,
        notifier: Arc<dyn ConfigNotifier>,
        access: Arc<dyn WorkspaceAccess>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            vault,
            validator,
            notifier,
            access,
            clock,
        }
    }

    /// OWNER/ADMIN of the workspace only.
    async fn authorize(&self, ws: WorkspaceId, user: UserId) -> AppResult<()> {
        match self.access.member_role(ws, user).await? {
            Some(WorkspaceRole::Owner | WorkspaceRole::Admin) => Ok(()),
            _ => Err(AppError::forbidden(
                "workspace owner or admin role required",
            )),
        }
    }

    fn require_vault(&self) -> AppResult<()> {
        if self.vault.is_enabled() {
            Ok(())
        } else {
            Err(AppError::coded(
                503,
                BYO_DISABLED,
                "bring-your-own LLM is disabled on this deployment (AI_KEYS_KEK is not set)",
            ))
        }
    }

    async fn view(&self, ws: WorkspaceId) -> AppResult<ConfigView> {
        Ok(ConfigView {
            byo_enabled: self.vault.is_enabled(),
            configs: self.repo.list_configs(ws).await?,
            credentials: self.repo.list_credentials(ws).await?,
        })
    }

    pub async fn get_config(&self, user: UserId, ws: WorkspaceId) -> AppResult<ConfigView> {
        self.authorize(ws, user).await?;
        self.view(ws).await
    }

    pub async fn put_config(
        &self,
        user: UserId,
        ws: WorkspaceId,
        cmd: PutConfigCommand,
    ) -> AppResult<PutConfigResult> {
        self.authorize(ws, user).await?;
        self.require_vault()?;

        // Validate everything before writing anything.
        let mut seen = std::collections::HashSet::new();
        let mut entries = Vec::with_capacity(cmd.entries.len());
        for (purpose, input) in cmd.entries {
            if !seen.insert(purpose) {
                return Err(AppError::validation(
                    "DUPLICATE_PURPOSE",
                    format!("purpose `{}` appears twice", purpose.as_str()),
                ));
            }
            let input = match input {
                None => None,
                Some(mut i) => {
                    i.model = i.model.trim().to_owned();
                    if i.model.is_empty() || i.model.len() > 200 {
                        return Err(AppError::validation(
                            "INVALID_MODEL",
                            "model must be 1-200 characters",
                        ));
                    }
                    i.base_url = clean_base_url(i.base_url)?;
                    if i.provider == LlmProviderKind::OpenAiCompatible && i.base_url.is_none() {
                        return Err(AppError::validation(
                            "INVALID_BASE_URL",
                            "baseUrl is required for openai_compatible",
                        ));
                    }
                    if let Some(u) = &i.base_url {
                        self.validator
                            .check_base_url(u)
                            .await
                            .map_err(|e| check_err(&e))?;
                    }
                    if i.data_boundary == BoundaryKind::WorkspaceControlled
                        && !cmd.acknowledge_workspace_controlled
                    {
                        return Err(AppError::validation(
                            "ACKNOWLEDGEMENT_REQUIRED",
                            "dataBoundary workspace_controlled requires acknowledgeWorkspaceControlled: true",
                        ));
                    }
                    if i.max_output_tokens.is_some_and(|v| v <= 0)
                        || i.timeout_ms.is_some_and(|v| v <= 0)
                    {
                        return Err(AppError::validation(
                            "INVALID_LIMIT",
                            "maxOutputTokens and timeoutMs must be positive",
                        ));
                    }
                    Some(i)
                }
            };
            entries.push((purpose, input));
        }

        let now = self.clock.now();
        let mut reindex_required = false;
        for (purpose, input) in entries {
            match input {
                Some(input) => {
                    if purpose == LlmPurpose::Embedding {
                        let old = self.repo.get_config(ws, purpose).await?;
                        reindex_required |= embedding_changed(old.as_ref(), &input);
                    }
                    let saved = self
                        .repo
                        .upsert_config(ws, purpose, &input, user, now)
                        .await?;
                    if input.data_boundary == BoundaryKind::WorkspaceControlled {
                        // Audit trail: who declared the endpoint tenant-controlled.
                        tracing::info!(
                            workspace_id = %ws, user_id = %user, purpose = purpose.as_str(),
                            provider = saved.provider.as_str(),
                            "audit: workspace_controlled data boundary declared"
                        );
                    }
                }
                None => {
                    if purpose == LlmPurpose::Embedding
                        && self.repo.get_config(ws, purpose).await?.is_some()
                    {
                        reindex_required = true;
                    }
                    self.repo.delete_config(ws, purpose).await?;
                }
            }
        }
        tracing::info!(workspace_id = %ws, user_id = %user, "audit: ai config updated");
        self.notifier.changed(ws).await;
        Ok(PutConfigResult {
            view: self.view(ws).await?,
            reindex_required,
        })
    }

    /// Base URL to validate against: explicit, else the workspace's configured one for the
    /// provider.
    async fn base_url_for(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        explicit: Option<String>,
    ) -> AppResult<Option<String>> {
        if explicit.is_some() {
            return Ok(explicit);
        }
        Ok(self
            .repo
            .list_configs(ws)
            .await?
            .into_iter()
            .find(|c| c.provider == provider && c.base_url.is_some())
            .and_then(|c| c.base_url))
    }

    /// Validates the key with the provider first; nothing is stored when that fails.
    pub async fn put_credential(
        &self,
        user: UserId,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        key: SecretString,
        base_url: Option<String>,
    ) -> AppResult<CredentialMeta> {
        self.authorize(ws, user).await?;
        self.require_vault()?;
        let raw = key.expose_secret();
        let trimmed = raw.trim();
        if trimmed.len() < MIN_API_KEY_LEN
            || trimmed.len() > MAX_API_KEY_LEN
            || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AppError::validation(
                "INVALID_API_KEY",
                "API key has an invalid format",
            ));
        }
        let key = if trimmed.len() == raw.len() {
            key
        } else {
            SecretString::from(trimmed.to_owned())
        };
        let base_url = self
            .base_url_for(ws, provider, clean_base_url(base_url)?)
            .await?;
        if let Some(u) = &base_url {
            self.validator
                .check_base_url(u)
                .await
                .map_err(|e| check_err(&e))?;
        }
        self.validator
            .validate(provider, base_url.as_deref(), &key)
            .await
            .map_err(|e| check_err(&e))?;

        let sealed = self.vault.seal(ws, provider, &key).map_err(vault_err)?;
        let now = self.clock.now();
        let meta = self
            .repo
            .upsert_credential(ws, provider, &sealed, user, now)
            .await?;
        tracing::info!(
            workspace_id = %ws, user_id = %user, provider = provider.as_str(),
            "audit: ai credential stored"
        );
        self.notifier.changed(ws).await;
        Ok(meta)
    }

    pub async fn delete_credential(
        &self,
        user: UserId,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<()> {
        self.authorize(ws, user).await?;
        if !self.repo.delete_credential(ws, provider).await? {
            return Err(AppError::not_found("no credential for this provider"));
        }
        tracing::info!(
            workspace_id = %ws, user_id = %user, provider = provider.as_str(),
            "audit: ai credential deleted"
        );
        self.notifier.changed(ws).await;
        Ok(())
    }

    /// Re-validates the stored key and records the outcome (status, `validated_at`).
    pub async fn test_credential(
        &self,
        user: UserId,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<TestOutcome> {
        self.authorize(ws, user).await?;
        self.require_vault()?;
        let stored = self
            .repo
            .get_credential(ws, provider)
            .await?
            .ok_or_else(|| AppError::not_found("no credential for this provider"))?;
        let key = self
            .vault
            .open(ws, provider, &stored.sealed)
            .map_err(vault_err)?;
        let base_url = self.base_url_for(ws, provider, None).await?;
        let started = Instant::now();
        let result = self
            .validator
            .validate(provider, base_url.as_deref(), &key)
            .await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let error_code = result.as_ref().err().map(error_code);
        // Transient outages do not flip the status.
        if !matches!(result, Err(KeyCheckError::Unreachable(_))) {
            self.repo
                .mark_validation(ws, provider, error_code, self.clock.now())
                .await?;
            self.notifier.changed(ws).await;
        }
        Ok(TestOutcome {
            ok: result.is_ok(),
            latency_ms,
            error_code,
        })
    }
}
