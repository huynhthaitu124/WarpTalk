//! `assistant.workspace_llm_*` over runtime sqlx queries. Every query filters by `workspace_id`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};

use crate::application::ports::LlmConfigRepository;
use crate::domain::byo::{
    BoundaryKind, ConfigStatus, CredentialMeta, FallbackPolicy, LlmConfig, LlmConfigInput,
    LlmProviderKind, LlmPurpose, ReasoningLevel, SealedSecret, StoredCredential,
};

const CONFIG_COLS: &str = "id, workspace_id, purpose, provider, model, base_url, reasoning, \
     max_output_tokens, timeout_ms, data_boundary, fallback_policy, enabled, status, \
     last_validated_at, last_error_code, version";

const CRED_COLS: &str = "id, workspace_id, provider, ciphertext, nonce, wrapped_dek, key_version, \
     last4, created_at, rotated_at, validated_at";

#[derive(sqlx::FromRow)]
struct ConfigRow {
    id: Uuid,
    workspace_id: Uuid,
    purpose: String,
    provider: String,
    model: String,
    base_url: Option<String>,
    reasoning: Option<String>,
    max_output_tokens: Option<i32>,
    timeout_ms: Option<i32>,
    data_boundary: String,
    fallback_policy: String,
    enabled: bool,
    status: String,
    last_validated_at: Option<DateTime<Utc>>,
    last_error_code: Option<String>,
    version: i64,
}

fn corrupt(what: &str, v: &str) -> AppError {
    AppError::internal(format!(
        "unexpected {what} `{v}` in assistant.workspace_llm_*"
    ))
}

impl TryFrom<ConfigRow> for LlmConfig {
    type Error = AppError;

    fn try_from(r: ConfigRow) -> AppResult<Self> {
        Ok(Self {
            id: r.id,
            workspace_id: WorkspaceId::from_uuid(r.workspace_id),
            purpose: LlmPurpose::parse(&r.purpose).ok_or_else(|| corrupt("purpose", &r.purpose))?,
            provider: LlmProviderKind::parse(&r.provider)
                .ok_or_else(|| corrupt("provider", &r.provider))?,
            model: r.model,
            base_url: r.base_url,
            reasoning: match r.reasoning {
                Some(s) => Some(ReasoningLevel::parse(&s).ok_or_else(|| corrupt("reasoning", &s))?),
                None => None,
            },
            max_output_tokens: r.max_output_tokens,
            timeout_ms: r.timeout_ms,
            data_boundary: BoundaryKind::parse(&r.data_boundary)
                .ok_or_else(|| corrupt("data_boundary", &r.data_boundary))?,
            fallback_policy: FallbackPolicy::parse(&r.fallback_policy)
                .ok_or_else(|| corrupt("fallback_policy", &r.fallback_policy))?,
            enabled: r.enabled,
            status: ConfigStatus::parse(&r.status).ok_or_else(|| corrupt("status", &r.status))?,
            last_validated_at: r.last_validated_at,
            last_error_code: r.last_error_code,
            version: r.version,
        })
    }
}

#[derive(sqlx::FromRow)]
struct CredRow {
    id: Uuid,
    workspace_id: Uuid,
    provider: String,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_dek: Vec<u8>,
    key_version: i32,
    last4: String,
    created_at: DateTime<Utc>,
    rotated_at: Option<DateTime<Utc>>,
    validated_at: Option<DateTime<Utc>>,
}

impl TryFrom<CredRow> for StoredCredential {
    type Error = AppError;

    fn try_from(r: CredRow) -> AppResult<Self> {
        Ok(Self {
            meta: CredentialMeta {
                id: r.id,
                workspace_id: WorkspaceId::from_uuid(r.workspace_id),
                provider: LlmProviderKind::parse(&r.provider)
                    .ok_or_else(|| corrupt("provider", &r.provider))?,
                last4: r.last4.clone(),
                created_at: r.created_at,
                rotated_at: r.rotated_at,
                validated_at: r.validated_at,
            },
            sealed: SealedSecret {
                ciphertext: r.ciphertext,
                nonce: r.nonce,
                wrapped_dek: r.wrapped_dek,
                key_version: r.key_version,
                last4: r.last4,
            },
        })
    }
}

#[derive(Clone)]
pub struct PgLlmConfigRepository {
    pool: PgPool,
}

impl PgLlmConfigRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LlmConfigRepository for PgLlmConfigRepository {
    async fn list_configs(&self, ws: WorkspaceId) -> AppResult<Vec<LlmConfig>> {
        let rows: Vec<ConfigRow> = sqlx::query_as(&format!(
            "SELECT {CONFIG_COLS} FROM assistant.workspace_llm_configs \
             WHERE workspace_id = $1 ORDER BY purpose"
        ))
        .bind(ws.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.into_iter().map(LlmConfig::try_from).collect()
    }

    async fn get_config(
        &self,
        ws: WorkspaceId,
        purpose: LlmPurpose,
    ) -> AppResult<Option<LlmConfig>> {
        let row: Option<ConfigRow> = sqlx::query_as(&format!(
            "SELECT {CONFIG_COLS} FROM assistant.workspace_llm_configs \
             WHERE workspace_id = $1 AND purpose = $2"
        ))
        .bind(ws.as_uuid())
        .bind(purpose.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.map(LlmConfig::try_from).transpose()
    }

    async fn upsert_config(
        &self,
        ws: WorkspaceId,
        purpose: LlmPurpose,
        i: &LlmConfigInput,
        by: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<LlmConfig> {
        let row: ConfigRow = sqlx::query_as(&format!(
            "INSERT INTO assistant.workspace_llm_configs \
               (id, workspace_id, purpose, provider, model, base_url, reasoning, \
                max_output_tokens, timeout_ms, data_boundary, fallback_policy, enabled, \
                status, last_validated_at, last_error_code, created_by, created_at, updated_at) \
             SELECT $1::uuid, $2::uuid, $3::text, $4::text, $5::text, $6::text, $7::text, \
                    $8::int, $9::int, $10::text, $11::text, $12::bool, \
                    CASE WHEN c.id IS NULL THEN 'invalid' ELSE 'ok' END, \
                    c.validated_at, \
                    CASE WHEN c.id IS NULL THEN 'NO_CREDENTIAL' END, \
                    $13::uuid, $14::timestamptz, $14::timestamptz \
             FROM (SELECT 1) s \
             LEFT JOIN assistant.workspace_llm_credentials c \
               ON c.workspace_id = $2 AND c.provider = $4 \
             ON CONFLICT (workspace_id, purpose) DO UPDATE SET \
               provider = EXCLUDED.provider, model = EXCLUDED.model, \
               base_url = EXCLUDED.base_url, reasoning = EXCLUDED.reasoning, \
               max_output_tokens = EXCLUDED.max_output_tokens, timeout_ms = EXCLUDED.timeout_ms, \
               data_boundary = EXCLUDED.data_boundary, \
               fallback_policy = EXCLUDED.fallback_policy, enabled = EXCLUDED.enabled, \
               status = EXCLUDED.status, last_validated_at = EXCLUDED.last_validated_at, \
               last_error_code = EXCLUDED.last_error_code, \
               version = assistant.workspace_llm_configs.version + 1, \
               updated_at = EXCLUDED.updated_at \
             RETURNING {CONFIG_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(ws.as_uuid())
        .bind(purpose.as_str())
        .bind(i.provider.as_str())
        .bind(&i.model)
        .bind(&i.base_url)
        .bind(i.reasoning.map(ReasoningLevel::as_str))
        .bind(i.max_output_tokens)
        .bind(i.timeout_ms)
        .bind(i.data_boundary.as_str())
        .bind(i.fallback_policy.as_str())
        .bind(i.enabled)
        .bind(by.as_uuid())
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.try_into()
    }

    async fn delete_config(&self, ws: WorkspaceId, purpose: LlmPurpose) -> AppResult<bool> {
        let r = sqlx::query(
            "DELETE FROM assistant.workspace_llm_configs WHERE workspace_id = $1 AND purpose = $2",
        )
        .bind(ws.as_uuid())
        .bind(purpose.as_str())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_credentials(&self, ws: WorkspaceId) -> AppResult<Vec<CredentialMeta>> {
        let rows: Vec<CredRow> = sqlx::query_as(&format!(
            "SELECT {CRED_COLS} FROM assistant.workspace_llm_credentials \
             WHERE workspace_id = $1 ORDER BY provider"
        ))
        .bind(ws.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.into_iter()
            .map(|r| StoredCredential::try_from(r).map(|c| c.meta))
            .collect()
    }

    async fn get_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<Option<StoredCredential>> {
        let row: Option<CredRow> = sqlx::query_as(&format!(
            "SELECT {CRED_COLS} FROM assistant.workspace_llm_credentials \
             WHERE workspace_id = $1 AND provider = $2"
        ))
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.map(StoredCredential::try_from).transpose()
    }

    async fn upsert_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        s: &SealedSecret,
        by: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<CredentialMeta> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let row: CredRow = sqlx::query_as(&format!(
            "INSERT INTO assistant.workspace_llm_credentials \
               (id, workspace_id, provider, ciphertext, nonce, wrapped_dek, key_version, last4, \
                created_by, created_at, validated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10) \
             ON CONFLICT (workspace_id, provider) DO UPDATE SET \
               ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce, \
               wrapped_dek = EXCLUDED.wrapped_dek, key_version = EXCLUDED.key_version, \
               last4 = EXCLUDED.last4, rotated_at = EXCLUDED.created_at, \
               validated_at = EXCLUDED.validated_at \
             RETURNING {CRED_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .bind(&s.ciphertext)
        .bind(&s.nonce)
        .bind(&s.wrapped_dek)
        .bind(s.key_version)
        .bind(&s.last4)
        .bind(by.as_uuid())
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE assistant.workspace_llm_configs \
             SET status = 'ok', last_validated_at = $3, last_error_code = NULL, \
                 version = version + 1, updated_at = $3 \
             WHERE workspace_id = $1 AND provider = $2",
        )
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(StoredCredential::try_from(row)?.meta)
    }

    async fn delete_credential(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
    ) -> AppResult<bool> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let r = sqlx::query(
            "DELETE FROM assistant.workspace_llm_credentials \
             WHERE workspace_id = $1 AND provider = $2",
        )
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE assistant.workspace_llm_configs \
             SET status = 'invalid', last_error_code = 'NO_CREDENTIAL', \
                 version = version + 1, updated_at = now() \
             WHERE workspace_id = $1 AND provider = $2",
        )
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(r.rows_affected() > 0)
    }

    async fn mark_validation(
        &self,
        ws: WorkspaceId,
        provider: LlmProviderKind,
        error_code: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        if error_code.is_none() {
            sqlx::query(
                "UPDATE assistant.workspace_llm_credentials SET validated_at = $3 \
                 WHERE workspace_id = $1 AND provider = $2",
            )
            .bind(ws.as_uuid())
            .bind(provider.as_str())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        }
        sqlx::query(
            "UPDATE assistant.workspace_llm_configs \
             SET status = CASE WHEN $3::text IS NULL THEN 'ok' ELSE 'invalid' END, \
                 last_error_code = $3, last_validated_at = $4, \
                 version = version + 1, updated_at = $4 \
             WHERE workspace_id = $1 AND provider = $2",
        )
        .bind(ws.as_uuid())
        .bind(provider.as_str())
        .bind(error_code)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }
}
