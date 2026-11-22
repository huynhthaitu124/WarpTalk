use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

use crate::{ObjectStore, StorageError, StorageResult, validate_key};

/// Stores objects as files below a root directory. Keys are validated so nothing outside the
/// root can be read or written.
#[derive(Debug, Clone)]
pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, key: &str) -> StorageResult<PathBuf> {
        validate_key(key)?;
        Ok(key.split('/').fold(self.root.clone(), |p, seg| p.join(seg)))
    }
}

fn io(e: std::io::Error) -> StorageError {
    StorageError::Backend(e.to_string())
}

#[async_trait]
impl ObjectStore for LocalFsStore {
    fn provider(&self) -> &'static str {
        "local"
    }

    async fn put(&self, key: &str, bytes: Bytes, _content_type: &str) -> StorageResult<()> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io)?;
        }
        // Write then rename so readers never see a partial object.
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::write(&tmp, &bytes).await.map_err(io)?;
        if let Err(e) = tokio::fs::rename(&tmp, &path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(io(e));
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> StorageResult<Bytes> {
        let path = self.path_for(key)?;
        match tokio::fs::read(&path).await {
            Ok(b) => Ok(Bytes::from(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(key.to_owned()))
            }
            Err(e) => Err(io(e)),
        }
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    async fn presign_get(&self, key: &str, _ttl: Duration) -> StorageResult<Option<String>> {
        validate_key(key)?;
        Ok(None)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn store() -> (LocalFsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wt-storage-{}", uuid::Uuid::new_v4()));
        (LocalFsStore::new(&dir), dir)
    }

    #[tokio::test]
    async fn roundtrip_and_delete() {
        let (s, dir) = store();
        s.put("ws/doc/a.txt", Bytes::from_static(b"hello"), "text/plain")
            .await
            .unwrap();
        assert_eq!(&s.get("ws/doc/a.txt").await.unwrap()[..], b"hello");
        s.put("ws/doc/a.txt", Bytes::from_static(b"v2"), "text/plain")
            .await
            .unwrap();
        assert_eq!(&s.get("ws/doc/a.txt").await.unwrap()[..], b"v2");
        s.delete("ws/doc/a.txt").await.unwrap();
        s.delete("ws/doc/a.txt").await.unwrap();
        assert!(matches!(
            s.get("ws/doc/a.txt").await,
            Err(StorageError::NotFound(_))
        ));
        assert_eq!(
            s.presign_get("ws/doc/a.txt", Duration::from_secs(60))
                .await
                .unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rejects_traversal() {
        let (s, dir) = store();
        for key in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "a//b",
            "",
        ] {
            assert!(
                matches!(
                    s.put(key, Bytes::from_static(b"x"), "text/plain").await,
                    Err(StorageError::InvalidKey(_))
                ),
                "{key}"
            );
            assert!(matches!(s.get(key).await, Err(StorageError::InvalidKey(_))));
            assert!(matches!(
                s.delete(key).await,
                Err(StorageError::InvalidKey(_))
            ));
        }
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
