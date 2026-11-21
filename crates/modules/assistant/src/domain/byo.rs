//! Per-workspace LLM configuration (BYO-LLM, docs/architecture/byo-llm.md section 6).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::WorkspaceId;

/// Redis pub/sub channel announcing that a workspace's AI configuration changed. The payload is
/// a JSON [`ConfigChanged`].
pub const AI_CONFIG_CHANNEL: &str = "warptalk:ai-config:changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigChanged {
    pub workspace_id: Uuid,
}

macro_rules! text_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

text_enum! {
    /// What an LLM call is for.
    LlmPurpose {
        Assistant => "assistant",
        SuggestionDecide => "suggestion_decide",
        SuggestionGenerate => "suggestion_generate",
        Summary => "summary",
        Embedding => "embedding",
    }
}

text_enum! {
    /// Provider kinds a workspace may bring.
    LlmProviderKind {
        OpenAi => "openai",
        Anthropic => "anthropic",
        Gemini => "gemini",
        OpenAiCompatible => "openai_compatible",
    }
}

text_enum! {
    /// Where the provider processes data (byo-llm.md section 8).
    BoundaryKind {
        External => "external",
        WorkspaceControlled => "workspace_controlled",
    }
}

text_enum! {
    /// What to do when the workspace's own provider cannot serve (section 9).
    FallbackPolicy {
        None => "none",
        PlatformIfAllowed => "platform_if_allowed",
    }
}

text_enum! {
    ReasoningLevel {
        Off => "off",
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

text_enum! {
    ConfigStatus {
        Ok => "ok",
        Degraded => "degraded",
        Invalid => "invalid",
    }
}

impl From<crate::suggestion::Purpose> for LlmPurpose {
    fn from(p: crate::suggestion::Purpose) -> Self {
        match p {
            crate::suggestion::Purpose::SuggestionDecide => Self::SuggestionDecide,
            crate::suggestion::Purpose::SuggestionGenerate => Self::SuggestionGenerate,
        }
    }
}

impl From<BoundaryKind> for crate::suggestion::DataBoundary {
    fn from(b: BoundaryKind) -> Self {
        match b {
            BoundaryKind::External => Self::External,
            BoundaryKind::WorkspaceControlled => Self::WorkspaceControlled,
        }
    }
}

/// One stored per-purpose override.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub purpose: LlmPurpose,
    pub provider: LlmProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub max_output_tokens: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub data_boundary: BoundaryKind,
    pub fallback_policy: FallbackPolicy,
    pub enabled: bool,
    pub status: ConfigStatus,
    pub last_validated_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub version: i64,
}

/// Fields a caller may set on a per-purpose override.
#[derive(Debug, Clone)]
pub struct LlmConfigInput {
    pub provider: LlmProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub max_output_tokens: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub data_boundary: BoundaryKind,
    pub fallback_policy: FallbackPolicy,
    pub enabled: bool,
}

/// Envelope-encrypted API key as stored. Contains no plaintext; `last4` is for display only.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedSecret {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    /// Wrapped DEK, wrap nonce prepended.
    pub wrapped_dek: Vec<u8>,
    pub key_version: i32,
    pub last4: String,
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSecret")
            .field("key_version", &self.key_version)
            .field("last4", &self.last4)
            .finish_non_exhaustive()
    }
}

/// Stored credential without its secret material (what listings expose).
#[derive(Debug, Clone)]
pub struct CredentialMeta {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub provider: LlmProviderKind,
    pub last4: String,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub validated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct StoredCredential {
    pub meta: CredentialMeta,
    pub sealed: SealedSecret,
}

/// Whether switching `old` -> `new` for the embedding purpose invalidates the vector index.
pub fn embedding_changed(old: Option<&LlmConfig>, new: &LlmConfigInput) -> bool {
    match old {
        Some(o) => o.provider != new.provider || o.model != new.model,
        // First override: differs from the platform profile unless proven otherwise.
        None => true,
    }
}

/// Minimum length accepted for an API key (also keeps `last4` a small fraction of it).
pub const MIN_API_KEY_LEN: usize = 8;
pub const MAX_API_KEY_LEN: usize = 512;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_roundtrip() {
        for p in LlmPurpose::ALL {
            assert_eq!(LlmPurpose::parse(p.as_str()), Some(*p));
        }
        assert_eq!(LlmProviderKind::parse("nope"), None);
        assert_eq!(
            serde_json::to_string(&LlmPurpose::SuggestionDecide)
                .ok()
                .as_deref(),
            Some("\"suggestion_decide\"")
        );
    }

    #[test]
    fn sealed_secret_debug_hides_bytes() {
        let s = SealedSecret {
            ciphertext: vec![1, 2, 3],
            nonce: vec![4],
            wrapped_dek: vec![5],
            key_version: 1,
            last4: "abcd".into(),
        };
        let d = format!("{s:?}");
        assert!(!d.contains("[1, 2, 3]"));
    }
}
