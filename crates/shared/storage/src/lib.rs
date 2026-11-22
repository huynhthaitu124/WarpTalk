//! Object storage for uploaded files: a small [`ObjectStore`] port with a local-filesystem
//! implementation (dev/tests) and a minimal S3-compatible one (MinIO, Cloudflare R2) that signs
//! requests with a hand-rolled AWS Signature V4.
//!
//! Selected at startup by `STORAGE_PROVIDER=local|s3` (see [`StorageConfig`]).

mod config;
mod local;
mod s3;
pub mod sigv4;

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

pub use config::{S3Config, StorageConfig, build_store};
pub use local::LocalFsStore;
pub use s3::S3Store;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The key is empty, absolute or contains traversal segments.
    #[error("invalid object key: {0}")]
    InvalidKey(String),
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("storage backend error: {0}")]
    Backend(String),
    #[error("storage configuration error: {0}")]
    Config(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Provider name stored next to the key (`local`, `s3`).
    fn provider(&self) -> &'static str;
    async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> StorageResult<()>;
    /// `NotFound` when the object does not exist.
    async fn get(&self, key: &str) -> StorageResult<Bytes>;
    /// Idempotent: deleting a missing object succeeds.
    async fn delete(&self, key: &str) -> StorageResult<()>;
    /// A time-limited direct download URL, or `None` when the backend cannot presign.
    async fn presign_get(&self, key: &str, ttl: Duration) -> StorageResult<Option<String>>;
}

/// Validates an object key: relative, `/`-separated, no empty / `.` / `..` segments, no
/// backslashes or control characters, at most 1024 bytes.
pub fn validate_key(key: &str) -> StorageResult<()> {
    let bad = |why: &str| Err(StorageError::InvalidKey(format!("{why}: {key:?}")));
    if key.is_empty() || key.len() > 1024 {
        return bad("empty or too long");
    }
    if key.contains('\\') || key.chars().any(char::is_control) {
        return bad("illegal character");
    }
    if key
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return bad("empty or traversal segment");
    }
    // A Windows drive prefix is absolute on some platforms.
    if key.len() >= 2 && key.as_bytes()[1] == b':' {
        return bad("drive prefix");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn key_validation() {
        for ok in ["a", "ws/doc/file.txt", "a-b_c.d/e"] {
            assert!(validate_key(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "/abs", "../x", "a/../b", "a//b", "a/./b", "a/", "a\\b", "c:/x", "a\0b",
        ] {
            assert!(validate_key(bad).is_err(), "{bad:?}");
        }
    }
}
