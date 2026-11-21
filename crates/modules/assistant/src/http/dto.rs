//! Request and response bodies (camelCase). Responses never carry key material, only `last4`.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use secrecy::SecretString;
use serde::{Deserialize, Deserializer, Serialize};
use warptalk_kernel::{AppError, AppResult};

use crate::application::ConfigView;
use crate::domain::byo::{
    BoundaryKind, CredentialMeta, FallbackPolicy, LlmConfig, LlmConfigInput, LlmProviderKind,
    LlmPurpose, ReasoningLevel,
};

fn de_secret<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
    Ok(SecretString::from(String::deserialize(d)?))
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurposeInput {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub max_output_tokens: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub data_boundary: Option<BoundaryKind>,
    pub fallback_policy: Option<FallbackPolicy>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutConfigRequest {
    /// Purpose -> override; `null` removes the override (platform default again).
    pub purposes: HashMap<String, Option<PurposeInput>>,
    /// Defaults for entries that do not set their own.
    pub base_url: Option<String>,
    pub fallback_policy: Option<FallbackPolicy>,
    pub data_boundary: Option<BoundaryKind>,
    #[serde(default)]
    pub acknowledge_workspace_controlled: bool,
}

pub fn parse_provider(s: &str) -> AppResult<LlmProviderKind> {
    LlmProviderKind::parse(s).ok_or_else(|| {
        AppError::validation(
            "INVALID_PROVIDER",
            "provider must be one of openai, anthropic, gemini, openai_compatible",
        )
    })
}

impl PutConfigRequest {
    pub fn into_entries(self) -> AppResult<Vec<(LlmPurpose, Option<LlmConfigInput>)>> {
        let mut out = Vec::with_capacity(self.purposes.len());
        for (name, input) in self.purposes {
            let purpose = LlmPurpose::parse(&name).ok_or_else(|| {
                AppError::validation("INVALID_PURPOSE", format!("unknown purpose `{name}`"))
            })?;
            let input = match input {
                None => None,
                Some(p) => Some(LlmConfigInput {
                    provider: parse_provider(&p.provider)?,
                    model: p.model,
                    base_url: p.base_url.or_else(|| self.base_url.clone()),
                    reasoning: p.reasoning,
                    max_output_tokens: p.max_output_tokens,
                    timeout_ms: p.timeout_ms,
                    data_boundary: p
                        .data_boundary
                        .or(self.data_boundary)
                        .unwrap_or(BoundaryKind::External),
                    fallback_policy: p
                        .fallback_policy
                        .or(self.fallback_policy)
                        .unwrap_or(FallbackPolicy::None),
                    enabled: p.enabled,
                }),
            };
            out.push((purpose, input));
        }
        Ok(out)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutCredentialRequest {
    #[serde(deserialize_with = "de_secret")]
    pub api_key: SecretString,
    /// Endpoint to validate against; defaults to the workspace's configured one.
    pub base_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurposeDto {
    pub provider: &'static str,
    pub model: String,
    pub base_url: Option<String>,
    pub reasoning: Option<&'static str>,
    pub max_output_tokens: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub data_boundary: &'static str,
    pub fallback_policy: &'static str,
    pub enabled: bool,
    pub status: &'static str,
    pub last_validated_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub version: i64,
}

impl From<&LlmConfig> for PurposeDto {
    fn from(c: &LlmConfig) -> Self {
        Self {
            provider: c.provider.as_str(),
            model: c.model.clone(),
            base_url: c.base_url.clone(),
            reasoning: c.reasoning.map(ReasoningLevel::as_str),
            max_output_tokens: c.max_output_tokens,
            timeout_ms: c.timeout_ms,
            data_boundary: c.data_boundary.as_str(),
            fallback_policy: c.fallback_policy.as_str(),
            enabled: c.enabled,
            status: c.status.as_str(),
            last_validated_at: c.last_validated_at,
            last_error_code: c.last_error_code.clone(),
            version: c.version,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDto {
    pub provider: &'static str,
    /// Last four characters of the key, for display only.
    pub last4: String,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub validated_at: Option<DateTime<Utc>>,
}

impl From<&CredentialMeta> for CredentialDto {
    fn from(c: &CredentialMeta) -> Self {
        Self {
            provider: c.provider.as_str(),
            last4: c.last4.clone(),
            created_at: c.created_at,
            rotated_at: c.rotated_at,
            validated_at: c.validated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    pub byo_enabled: bool,
    pub purposes: BTreeMap<&'static str, PurposeDto>,
    pub credentials: Vec<CredentialDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_required: Option<bool>,
}

impl ConfigResponse {
    pub fn new(v: &ConfigView, reindex_required: Option<bool>) -> Self {
        Self {
            byo_enabled: v.byo_enabled,
            purposes: v
                .configs
                .iter()
                .map(|c| (c.purpose.as_str(), PurposeDto::from(c)))
                .collect(),
            credentials: v.credentials.iter().map(CredentialDto::from).collect(),
            reindex_required,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResponse {
    pub ok: bool,
    pub latency_ms: u64,
    pub error_code: Option<&'static str>,
}
