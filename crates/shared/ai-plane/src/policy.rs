//! Workspace consent for sending data to third-party LLM endpoints.

use async_trait::async_trait;
use sqlx::PgPool;
use warptalk_kernel::WorkspaceId;

/// Fail-closed: `false` whenever the answer is unknown.
#[async_trait]
pub trait ExternalLlmPolicy: Send + Sync {
    async fn external_llm_allowed(&self, workspace: WorkspaceId) -> bool;
}

/// Fixed answer (tests, single-tenant deployments).
pub struct StaticExternalPolicy(pub bool);

#[async_trait]
impl ExternalLlmPolicy for StaticExternalPolicy {
    async fn external_llm_allowed(&self, _: WorkspaceId) -> bool {
        self.0
    }
}

/// Reads `workspace.workspaces.settings ->> 'externalLlmAllowed'` (jsonb boolean).
/// TEMPORARY cross-schema read, like the worker's other policy adapters.
pub struct PgExternalLlmPolicy {
    pool: PgPool,
}

impl PgExternalLlmPolicy {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ExternalLlmPolicy for PgExternalLlmPolicy {
    async fn external_llm_allowed(&self, workspace: WorkspaceId) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE(settings -> 'externalLlmAllowed' = 'true'::jsonb, false) \
             FROM workspace.workspaces WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(workspace.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| tracing::warn!(error = %e, "external LLM policy lookup failed; denying"))
        .ok()
        .flatten()
        .unwrap_or(false)
    }
}
