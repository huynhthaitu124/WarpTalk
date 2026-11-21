//! Embedding profile: (provider, model, dims, distance). Vectors of different profiles must
//! never share a collection, so the collection name embeds a hash of the profile.

use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfile {
    pub provider: String,
    pub model: String,
    pub dims: usize,
    /// `cosine` | `dot` | `euclid`
    pub distance: String,
}

impl EmbeddingProfile {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
        distance: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            dims,
            distance: distance.into(),
        }
    }

    /// First 12 hex chars of `sha256(provider|model|dims|distance)`.
    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        h.update(
            format!(
                "{}|{}|{}|{}",
                self.provider, self.model, self.dims, self.distance
            )
            .as_bytes(),
        );
        h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
    }

    /// `workspace_{id}_{profile_hash}`
    pub fn collection(&self, workspace: Uuid) -> String {
        format!("workspace_{}_{}", workspace, self.hash())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hash_changes_with_any_profile_field() {
        let a = EmbeddingProfile::new("openai", "m1", 8, "cosine");
        assert_eq!(a.hash().len(), 12);
        assert_eq!(a.hash(), a.clone().hash());
        for b in [
            EmbeddingProfile::new("other", "m1", 8, "cosine"),
            EmbeddingProfile::new("openai", "m2", 8, "cosine"),
            EmbeddingProfile::new("openai", "m1", 16, "cosine"),
            EmbeddingProfile::new("openai", "m1", 8, "dot"),
        ] {
            assert_ne!(a.hash(), b.hash());
        }
        let ws = Uuid::now_v7();
        assert_eq!(
            a.collection(ws),
            format!("workspace_{}_{}", ws, a.hash())
        );
    }
}
