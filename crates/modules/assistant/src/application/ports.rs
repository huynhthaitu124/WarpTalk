//! Ports of the BYO-LLM configuration use cases. `apps/api` wires the adapters.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};

use crate::domain::byo::{
    CredentialMeta, LlmConfig, LlmConfigInput, LlmProviderKind, LlmPurpose, SealedSecret,
    StoredCredential,
};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// No key-encryption key configured: BYO credentials cannot be stored or read.
    #[error("{0}")]
    Disabled(String),
    #[error("credential crypto failed: {0}")]
    Crypto(String),
}

/// Envelope encryption of workspace API keys. Implementations never log or print key material.
pub trait SecretVault: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn seal(
        &self,
        workspace: WorkspaceId,
        provider: LlmProviderKind,
        key: &SecretString,
    ) -> Result<SealedSecret, VaultError>;
    fn open(
        &self,
        workspace: WorkspaceId,
        provider: LlmProviderKind,
        sealed: &SealedSecret,
    ) -> Result<SecretString, VaultError>;
}

#[derive(Debug, thiserror::Error)]
pub enum KeyCheckError {
    /// The provider rejected the key (401/403) or the model does not exist for it.
    #[error("provider rejected the credentials: {0}")]
    Rejected(String),
    /// Quota or billing problem on the customer's account.
    #[error("provider account problem: {0}")]
    Account(String),
    /// Network failure, timeout or provider outage.
    #[error("provider unreachable: {0}")]
    Unreachable(String),
    #[error("base url not allowed: {0}")]
    BadBaseUrl(String),
}

/// Checks a key against the provider before it is stored (`LlmProvider::validate`).
#[async_trait]
pub trait KeyValidator: Send + Sync {
    /// SSRF guard for customer-supplied endpoints.
    async fn check_base_url(&self, base_url: &str) -> Result<(), KeyCheckError>;
    async fn validate(
        &self,
        provider: LlmProviderKind,
        base_url: Option<&str>,
        key: &SecretString,
    ) -> Result<(), KeyCheckError>;
}

/// Announces a configuration change to every replica (`AI_CONFIG_CHANNEL`).
#[async_trait]
pub trait ConfigNotifier: Send + Sync {
    async fn changed(&self, workspace: WorkspaceId);
}

pub struct NoopNotifier;

#[async_trait]
impl ConfigNotifier for NoopNotifier {
    async fn changed(&self, _: WorkspaceId) {}
}

/// Membership lookup, implemented by `apps/api` over the workspace module.
#[async_trait]
pub trait WorkspaceAccess: Send + Sync {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
}

/// Storage for configs and credentials. Every query is scoped by `workspace_id`.
#[async_trait]
pub trait LlmConfigRepository: Send + Sync {
    async fn list_configs(&self, ws: WorkspaceId) -> AppResult<Vec<LlmConfig>>;
    async fn get_config(
        &self,
        ws: WorkspaceId,
        purpose: LlmPurpose,
    ) -> AppResult<Option<LlmConfig>>;
    /// Insert or update; bumps `version`; status is `ok` when a credential for the provider
    /// exists, else `invalid` (`NO_CREDENTIAL`).
    async fn upsert_config(
        &self,
        ws: WorkspaceId,
        purpose: LlmPurpose,
        input: &LlmConfigInput,
        by: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<LlmConfig>;
    async fn delete_config(&self, ws: WorkspaceId, purpose: LlmPurpose) -> AppResult<bool>;

    async fn list_credentials(&self, ws: WorkspaceId) -> AppResult<Vec<CredentialMeta>>;
    async fn get_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<Option<StoredCredential>>;
    /// Insert or rotate; marks the provider's configs `ok` and bumps their versions.
    async fn upsert_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        sealed: &SealedSecret,
        by: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<CredentialMeta>;
    /// Removes the key; the provider's configs become `invalid` (`NO_CREDENTIAL`).
    async fn delete_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<bool>;
    /// Records a validation result on the credential and the provider's configs.
    async fn mark_validation(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        error_code: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<()>;
}
