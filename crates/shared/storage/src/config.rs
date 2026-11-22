use std::sync::Arc;

use crate::{LocalFsStore, ObjectStore, S3Store, StorageError, StorageResult};

pub const DEFAULT_LOCAL_DIR: &str = "./.data/objects";

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    /// Base url used in presigned links when clients reach the store under another address
    /// than the server (`S3_PUBLIC_ENDPOINT`); defaults to `endpoint`.
    pub public_endpoint: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StorageConfig {
    Local { dir: String },
    S3(S3Config),
}

impl StorageConfig {
    /// From `STORAGE_PROVIDER` (`local` default | `s3`), `STORAGE_LOCAL_DIR`, `S3_ENDPOINT`,
    /// `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, `S3_REGION` (default
    /// `us-east-1`; R2 uses `auto`), `S3_PUBLIC_ENDPOINT`.
    pub fn from_env() -> StorageResult<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> StorageResult<Self> {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let provider = get("STORAGE_PROVIDER").unwrap_or_else(|| "local".to_owned());
        match provider.to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local {
                dir: get("STORAGE_LOCAL_DIR").unwrap_or_else(|| DEFAULT_LOCAL_DIR.to_owned()),
            }),
            "s3" => {
                let need = |k: &str| {
                    get(k).ok_or_else(|| {
                        StorageError::Config(format!("{k} is required when STORAGE_PROVIDER=s3"))
                    })
                };
                Ok(Self::S3(S3Config {
                    endpoint: need("S3_ENDPOINT")?,
                    bucket: need("S3_BUCKET")?,
                    access_key_id: need("S3_ACCESS_KEY_ID")?,
                    secret_access_key: need("S3_SECRET_ACCESS_KEY")?,
                    region: get("S3_REGION").unwrap_or_else(|| "us-east-1".to_owned()),
                    public_endpoint: get("S3_PUBLIC_ENDPOINT"),
                }))
            }
            other => Err(StorageError::Config(format!(
                "unknown STORAGE_PROVIDER `{other}` (expected local or s3)"
            ))),
        }
    }
}

/// Builds the store for a configuration.
pub fn build_store(cfg: &StorageConfig) -> StorageResult<Arc<dyn ObjectStore>> {
    Ok(match cfg {
        StorageConfig::Local { dir } => Arc::new(LocalFsStore::new(dir.clone())),
        StorageConfig::S3(s3) => Arc::new(S3Store::new(s3.clone())?),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn defaults_to_local() {
        let StorageConfig::Local { dir } = StorageConfig::from_lookup(lookup(&[])).unwrap() else {
            panic!("expected local");
        };
        assert_eq!(dir, DEFAULT_LOCAL_DIR);
    }

    #[test]
    fn s3_needs_credentials() {
        let err = StorageConfig::from_lookup(lookup(&[("STORAGE_PROVIDER", "s3")])).unwrap_err();
        assert!(err.to_string().contains("S3_ENDPOINT"));
        let cfg = StorageConfig::from_lookup(lookup(&[
            ("STORAGE_PROVIDER", "S3"),
            ("S3_ENDPOINT", "http://minio:9000"),
            ("S3_BUCKET", "b"),
            ("S3_ACCESS_KEY_ID", "k"),
            ("S3_SECRET_ACCESS_KEY", "s"),
        ]))
        .unwrap();
        assert!(matches!(cfg, StorageConfig::S3(c) if c.region == "us-east-1"));
        assert!(StorageConfig::from_lookup(lookup(&[("STORAGE_PROVIDER", "gcs")])).is_err());
    }
}
