//! Envelope encryption of workspace API keys (docs/architecture/byo-llm.md section 6.2).
//!
//! A random 256-bit DEK per credential encrypts the key with AES-256-GCM; the DEK is wrapped by
//! the platform KEK (`AI_KEYS_KEK`, base64 of 32 bytes) with AES-256-GCM too (wrap nonce
//! prepended). Both use AAD = `workspace_id (16 bytes) || provider`, so a ciphertext copied to
//! another workspace or provider fails to open. Plaintext lives in `SecretString` / `Zeroizing`
//! and is wiped on drop. Nothing here prints or serializes key material.

use std::sync::Arc;

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use rand::rngs::OsRng;
use secrecy::{ExposeSecret, SecretString};
use warptalk_assistant::application::ports::{SecretVault, VaultError};
use warptalk_assistant::domain::byo::{LlmProviderKind, SealedSecret};
use warptalk_kernel::WorkspaceId;
use zeroize::{Zeroize, Zeroizing};

pub const ENV_KEK: &str = "AI_KEYS_KEK";
pub const ENV_KEK_VERSION: &str = "AI_KEYS_KEK_VERSION";

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

pub struct KeyVault {
    kek: Zeroizing<[u8; KEY_LEN]>,
    version: i32,
}

impl std::fmt::Debug for KeyVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVault")
            .field("kek", &"[REDACTED]")
            .field("version", &self.version)
            .finish()
    }
}

fn aad(ws: WorkspaceId, provider: LlmProviderKind) -> Vec<u8> {
    let mut v = ws.as_uuid().as_bytes().to_vec();
    v.extend_from_slice(provider.as_str().as_bytes());
    v
}

fn crypto(what: &str) -> VaultError {
    // Deliberately terse: AEAD errors carry no detail and nothing secret may leak here.
    VaultError::Crypto(what.to_owned())
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

impl KeyVault {
    /// `kek_b64` is the standard-base64 encoding of exactly 32 bytes.
    pub fn from_base64(kek_b64: &str, version: i32) -> Result<Self, VaultError> {
        let mut raw = STANDARD
            .decode(kek_b64.trim())
            .map_err(|_| VaultError::Crypto(format!("{ENV_KEK} is not valid base64")))?;
        let mut kek = Zeroizing::new([0u8; KEY_LEN]);
        let ok = raw.len() == KEY_LEN;
        if ok {
            kek.copy_from_slice(&raw);
        }
        raw.zeroize();
        if !ok {
            return Err(VaultError::Crypto(format!(
                "{ENV_KEK} must decode to exactly {KEY_LEN} bytes"
            )));
        }
        Ok(Self { kek, version })
    }

    /// `AI_KEYS_KEK` (required) and `AI_KEYS_KEK_VERSION` (default 1).
    /// Missing or blank KEK is [`VaultError::Disabled`].
    pub fn from_env() -> Result<Self, VaultError> {
        let kek = std::env::var(ENV_KEK)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                VaultError::Disabled(format!(
                    "{ENV_KEK} is not set (base64 of 32 random bytes): bring-your-own LLM is disabled"
                ))
            })?;
        let version = std::env::var(ENV_KEK_VERSION)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        Self::from_base64(&kek, version)
    }

    pub fn version(&self) -> i32 {
        self.version
    }
}

impl SecretVault for KeyVault {
    fn is_enabled(&self) -> bool {
        true
    }

    fn seal(
        &self,
        workspace: WorkspaceId,
        provider: LlmProviderKind,
        key: &SecretString,
    ) -> Result<SealedSecret, VaultError> {
        let plain = key.expose_secret();
        let aad = aad(workspace, provider);
        let dek = Zeroizing::new(random::<KEY_LEN>());
        let nonce = random::<NONCE_LEN>();
        let wrap_nonce = random::<NONCE_LEN>();

        let ciphertext = Aes256Gcm::new_from_slice(dek.as_slice())
            .map_err(|_| crypto("bad data key"))?
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| crypto("encryption failed"))?;
        let wrapped = Aes256Gcm::new_from_slice(self.kek.as_slice())
            .map_err(|_| crypto("bad key-encryption key"))?
            .encrypt(
                Nonce::from_slice(&wrap_nonce),
                Payload {
                    msg: dek.as_slice(),
                    aad: &aad,
                },
            )
            .map_err(|_| crypto("key wrap failed"))?;
        let mut wrapped_dek = wrap_nonce.to_vec();
        wrapped_dek.extend_from_slice(&wrapped);

        let last4: String = {
            let chars: Vec<char> = plain.chars().collect();
            chars[chars.len().saturating_sub(4)..].iter().collect()
        };
        Ok(SealedSecret {
            ciphertext,
            nonce: nonce.to_vec(),
            wrapped_dek,
            key_version: self.version,
            last4,
        })
    }

    fn open(
        &self,
        workspace: WorkspaceId,
        provider: LlmProviderKind,
        sealed: &SealedSecret,
    ) -> Result<SecretString, VaultError> {
        if sealed.key_version != self.version {
            return Err(VaultError::Crypto(format!(
                "credential was sealed with KEK version {}, this process has version {}",
                sealed.key_version, self.version
            )));
        }
        if sealed.wrapped_dek.len() <= NONCE_LEN || sealed.nonce.len() != NONCE_LEN {
            return Err(crypto("malformed credential"));
        }
        let aad = aad(workspace, provider);
        let (wrap_nonce, wrapped) = sealed.wrapped_dek.split_at(NONCE_LEN);
        let dek = Zeroizing::new(
            Aes256Gcm::new_from_slice(self.kek.as_slice())
                .map_err(|_| crypto("bad key-encryption key"))?
                .decrypt(
                    Nonce::from_slice(wrap_nonce),
                    Payload {
                        msg: wrapped,
                        aad: &aad,
                    },
                )
                .map_err(|_| crypto("credential does not match this workspace or provider"))?,
        );
        let plain = Aes256Gcm::new_from_slice(dek.as_slice())
            .map_err(|_| crypto("bad data key"))?
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| crypto("decryption failed"))?;
        match String::from_utf8(plain) {
            Ok(s) => Ok(SecretString::from(s)),
            Err(e) => {
                let mut bytes = e.into_bytes();
                bytes.zeroize();
                Err(crypto("decrypted credential is not text"))
            }
        }
    }
}

/// Stand-in when no KEK is configured: BYO stays off with a clear error.
pub struct DisabledVault(pub String);

impl SecretVault for DisabledVault {
    fn is_enabled(&self) -> bool {
        false
    }

    fn seal(
        &self,
        _: WorkspaceId,
        _: LlmProviderKind,
        _: &SecretString,
    ) -> Result<SealedSecret, VaultError> {
        Err(VaultError::Disabled(self.0.clone()))
    }

    fn open(
        &self,
        _: WorkspaceId,
        _: LlmProviderKind,
        _: &SealedSecret,
    ) -> Result<SecretString, VaultError> {
        Err(VaultError::Disabled(self.0.clone()))
    }
}

/// Vault from the environment. Without `AI_KEYS_KEK` (or with an invalid one) BYO is disabled
/// and the reason is logged once.
pub fn vault_from_env() -> Arc<dyn SecretVault> {
    match KeyVault::from_env() {
        Ok(v) => Arc::new(v),
        Err(e) => {
            tracing::warn!(error = %e, "BYO LLM credentials disabled");
            Arc::new(DisabledVault(e.to_string()))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn vault() -> KeyVault {
        KeyVault::from_base64(&STANDARD.encode([7u8; 32]), 1).unwrap()
    }

    #[test]
    fn roundtrip_and_last4() {
        let v = vault();
        let ws = WorkspaceId::new();
        let key = SecretString::from("sk-test-abcdef1234");
        let sealed = v.seal(ws, LlmProviderKind::OpenAi, &key).unwrap();
        assert_eq!(sealed.last4, "1234");
        assert!(!sealed.ciphertext.windows(10).any(|w| w == b"sk-test-ab"));
        let out = v.open(ws, LlmProviderKind::OpenAi, &sealed).unwrap();
        assert_eq!(out.expose_secret(), "sk-test-abcdef1234");
    }

    #[test]
    fn sealing_twice_differs() {
        let v = vault();
        let ws = WorkspaceId::new();
        let key = SecretString::from("sk-test-abcdef1234");
        let a = v.seal(ws, LlmProviderKind::OpenAi, &key).unwrap();
        let b = v.seal(ws, LlmProviderKind::OpenAi, &key).unwrap();
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.wrapped_dek, b.wrapped_dek);
    }

    #[test]
    fn wrong_aad_fails() {
        let v = vault();
        let ws = WorkspaceId::new();
        let sealed = v
            .seal(
                ws,
                LlmProviderKind::OpenAi,
                &SecretString::from("sk-test-abcdef1234"),
            )
            .unwrap();
        assert!(
            v.open(WorkspaceId::new(), LlmProviderKind::OpenAi, &sealed)
                .is_err()
        );
        assert!(v.open(ws, LlmProviderKind::Anthropic, &sealed).is_err());
    }

    #[test]
    fn wrong_kek_or_tampering_fails() {
        let v = vault();
        let ws = WorkspaceId::new();
        let mut sealed = v
            .seal(
                ws,
                LlmProviderKind::Gemini,
                &SecretString::from("sk-test-abcdef1234"),
            )
            .unwrap();
        let other = KeyVault::from_base64(&STANDARD.encode([9u8; 32]), 1).unwrap();
        assert!(other.open(ws, LlmProviderKind::Gemini, &sealed).is_err());
        sealed.ciphertext[0] ^= 1;
        assert!(v.open(ws, LlmProviderKind::Gemini, &sealed).is_err());
    }

    #[test]
    fn debug_never_contains_key_material() {
        let v = vault();
        let ws = WorkspaceId::new();
        let key = SecretString::from("sk-test-SECRETSECRET9999");
        let sealed = v.seal(ws, LlmProviderKind::OpenAi, &key).unwrap();
        for dbg in [format!("{v:?}"), format!("{sealed:?}"), format!("{key:?}")] {
            assert!(!dbg.contains("SECRETSECRET"), "{dbg}");
        }
        let kek = STANDARD.encode([7u8; 32]);
        assert!(!format!("{v:?}").contains(&kek));
        let opened = v.open(ws, LlmProviderKind::OpenAi, &sealed).unwrap();
        assert!(!format!("{opened:?}").contains("SECRETSECRET"));
    }

    #[test]
    fn rejects_bad_kek() {
        assert!(KeyVault::from_base64("not base64!!", 1).is_err());
        assert!(KeyVault::from_base64(&STANDARD.encode([1u8; 16]), 1).is_err());
    }

    #[test]
    fn disabled_vault_reports_clear_error() {
        let d = DisabledVault("AI_KEYS_KEK is not set".into());
        assert!(!d.is_enabled());
        let e = d
            .seal(
                WorkspaceId::new(),
                LlmProviderKind::OpenAi,
                &SecretString::from("x"),
            )
            .unwrap_err();
        assert!(e.to_string().contains("AI_KEYS_KEK"));
    }
}
