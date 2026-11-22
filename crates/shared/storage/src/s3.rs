use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};

use crate::config::S3Config;
use crate::sigv4::{self, Credentials, Scope};
use crate::{ObjectStore, StorageError, StorageResult, validate_key};

const SERVICE: &str = "s3";
/// Longest presigned lifetime SigV4 allows (7 days).
const MAX_PRESIGN_SECS: u64 = 604_800;

/// Minimal S3 client (path-style addressing: `{endpoint}/{bucket}/{key}`), enough for MinIO and
/// Cloudflare R2. No multipart upload: objects are sent in one PUT.
pub struct S3Store {
    http: reqwest::Client,
    cfg: S3Config,
    endpoint: url::Url,
    public_endpoint: url::Url,
}

fn parse_endpoint(raw: &str) -> StorageResult<url::Url> {
    let u = url::Url::parse(raw.trim_end_matches('/'))
        .map_err(|e| StorageError::Config(format!("invalid S3 endpoint `{raw}`: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(StorageError::Config(format!(
            "S3 endpoint must be an http(s) url: `{raw}`"
        )));
    }
    Ok(u)
}

/// `host[:port]` as it appears in the `Host` header.
fn authority(u: &url::Url) -> String {
    let host = u.host_str().unwrap_or_default();
    match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    }
}

fn amz_date(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

impl S3Store {
    pub fn new(cfg: S3Config) -> StorageResult<Self> {
        let endpoint = parse_endpoint(&cfg.endpoint)?;
        let public_endpoint = match &cfg.public_endpoint {
            Some(p) => parse_endpoint(p)?,
            None => endpoint.clone(),
        };
        if cfg.bucket.is_empty() || cfg.bucket.contains('/') {
            return Err(StorageError::Config("invalid S3 bucket name".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| StorageError::Config(e.to_string()))?;
        Ok(Self {
            http,
            cfg,
            endpoint,
            public_endpoint,
        })
    }

    fn creds(&self) -> Credentials<'_> {
        Credentials {
            access_key_id: &self.cfg.access_key_id,
            secret_access_key: &self.cfg.secret_access_key,
        }
    }

    fn scope(&self) -> Scope<'_> {
        Scope {
            region: &self.cfg.region,
            service: SERVICE,
        }
    }

    /// Path (without a base path of the endpoint) of an object, encoded for the canonical URI.
    fn canonical_uri(&self, base: &url::Url, key: &str) -> String {
        let prefix = base.path().trim_end_matches('/');
        format!(
            "{prefix}/{}/{}",
            sigv4::uri_encode(&self.cfg.bucket, false),
            sigv4::uri_encode(key, true)
        )
    }

    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Bytes,
        content_type: Option<&str>,
        now: DateTime<Utc>,
    ) -> StorageResult<reqwest::Response> {
        validate_key(key)?;
        let uri = self.canonical_uri(&self.endpoint, key);
        let host = authority(&self.endpoint);
        let date = amz_date(now);
        let payload_hash = sigv4::sha256_hex(&body);
        let auth = sigv4::authorization_header(
            method.as_str(),
            &uri,
            "",
            &[
                ("host", host.as_str()),
                ("x-amz-content-sha256", payload_hash.as_str()),
                ("x-amz-date", date.as_str()),
            ],
            &payload_hash,
            &date,
            self.creds(),
            self.scope(),
        );
        let mut url = self.endpoint.clone();
        url.set_path(&uri);
        let mut req = self
            .http
            .request(method, url)
            .header("x-amz-date", &date)
            .header("x-amz-content-sha256", &payload_hash)
            .header("authorization", auth);
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        req.body(body)
            .send()
            .await
            .map_err(|e| StorageError::Backend(format!("s3 request failed: {e}")))
    }
}

async fn failure(resp: reqwest::Response, what: &str) -> StorageError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    StorageError::Backend(format!("s3 {what} failed: {status} {body}"))
}

#[async_trait]
impl ObjectStore for S3Store {
    fn provider(&self) -> &'static str {
        "s3"
    }

    async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> StorageResult<()> {
        let resp = self
            .send(
                reqwest::Method::PUT,
                key,
                bytes,
                Some(content_type),
                Utc::now(),
            )
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(failure(resp, "put").await)
        }
    }

    async fn get(&self, key: &str) -> StorageResult<Bytes> {
        let resp = self
            .send(reqwest::Method::GET, key, Bytes::new(), None, Utc::now())
            .await?;
        match resp.status() {
            s if s.is_success() => resp
                .bytes()
                .await
                .map_err(|e| StorageError::Backend(format!("s3 get body failed: {e}"))),
            reqwest::StatusCode::NOT_FOUND => Err(StorageError::NotFound(key.to_owned())),
            _ => Err(failure(resp, "get").await),
        }
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        let resp = self
            .send(reqwest::Method::DELETE, key, Bytes::new(), None, Utc::now())
            .await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(failure(resp, "delete").await)
        }
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> StorageResult<Option<String>> {
        validate_key(key)?;
        let secs = ttl.as_secs().clamp(1, MAX_PRESIGN_SECS);
        let uri = self.canonical_uri(&self.public_endpoint, key);
        let query = sigv4::presigned_query(
            "GET",
            &uri,
            &authority(&self.public_endpoint),
            &amz_date(Utc::now()),
            secs,
            self.creds(),
            self.scope(),
        );
        let mut url = self.public_endpoint.clone();
        url.set_path(&uri);
        // `set_path` keeps already-encoded sequences; the query is already canonical.
        url.set_query(Some(&query));
        Ok(Some(url.into()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn store() -> S3Store {
        S3Store::new(S3Config {
            endpoint: "http://localhost:9000".into(),
            bucket: "warptalk".into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
            public_endpoint: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn presigned_url_shape() {
        let s = store();
        let u = s
            .presign_get("ws/doc 1/a.txt", Duration::from_secs(300))
            .await
            .unwrap()
            .unwrap();
        assert!(
            u.starts_with("http://localhost:9000/warptalk/ws/doc%201/a.txt?X-Amz-Algorithm="),
            "{u}"
        );
        assert!(u.contains("X-Amz-Expires=300"));
        assert!(u.contains("X-Amz-Signature="));
        assert!(s.presign_get("../x", Duration::from_secs(1)).await.is_err());
    }

    #[test]
    fn rejects_bad_config() {
        let mut c = S3Config {
            endpoint: "ftp://x".into(),
            bucket: "b".into(),
            access_key_id: "a".into(),
            secret_access_key: "s".into(),
            region: "r".into(),
            public_endpoint: None,
        };
        assert!(S3Store::new(c.clone()).is_err());
        c.endpoint = "http://x:9000".into();
        c.bucket = "a/b".into();
        assert!(S3Store::new(c).is_err());
    }
}
