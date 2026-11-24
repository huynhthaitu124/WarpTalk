//! Google: ID token verification (Sign in with Google), Calendar/Meet event creation.
//!
//! [`GoogleVerifier`] validates an ID token locally: RS256 signature against Google's published
//! JWKS (cached per `Cache-Control: max-age`), issuer, audience (one of the configured client
//! ids, BR-04), expiry and a verified email.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

pub const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUERS: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];
/// Used when the JWKS response has no usable `max-age`.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);
const MAX_TTL: Duration = Duration::from_secs(24 * 3600);
/// An unknown `kid` forces a refetch, but at most this often (stops token-spam amplification).
const MIN_REFETCH: Duration = Duration::from_secs(30);

/// A verified Google identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleIdentity {
    /// Stable Google account id (`sub`).
    pub sub: String,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GoogleError {
    /// The token must be rejected (401 to the client).
    #[error("invalid google id token: {0}")]
    InvalidToken(String),
    /// Google's key endpoint could not be reached (retryable, 503/502 to the client).
    #[error("google keys unavailable: {0}")]
    Unavailable(String),
    #[error("google configuration error: {0}")]
    Config(String),
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    email: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string")]
    email_verified: bool,
    name: Option<String>,
    picture: Option<String>,
}

/// Google historically sent `email_verified` as either a bool or the strings "true"/"false".
fn bool_or_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        B(bool),
        S(String),
    }
    Ok(match V::deserialize(d)? {
        V::B(b) => b,
        V::S(s) => s.eq_ignore_ascii_case("true"),
    })
}

struct KeyCache {
    keys: Vec<(String, DecodingKey)>,
    fetched_at: Instant,
    expires_at: Instant,
}

pub struct GoogleVerifier {
    http: reqwest::Client,
    client_ids: Vec<String>,
    jwks_url: String,
    cache: Mutex<Option<KeyCache>>,
}

impl GoogleVerifier {
    /// `client_ids`: every OAuth client id allowed as `aud` (web, macOS, ...).
    pub fn new(client_ids: Vec<String>) -> Result<Self, GoogleError> {
        let client_ids: Vec<String> = client_ids
            .into_iter()
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty())
            .collect();
        if client_ids.is_empty() {
            return Err(GoogleError::Config("no google client ids".to_owned()));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| GoogleError::Config(format!("http client: {e}")))?;
        Ok(Self {
            http,
            client_ids,
            jwks_url: GOOGLE_JWKS_URL.to_owned(),
            cache: Mutex::new(None),
        })
    }

    /// Parses a comma-separated `GOOGLE_CLIENT_IDS` value; `None` when it has no ids.
    pub fn from_client_ids_csv(csv: &str) -> Option<Result<Self, GoogleError>> {
        let ids: Vec<String> = csv.split(',').map(str::to_owned).collect();
        if ids.iter().all(|i| i.trim().is_empty()) {
            None
        } else {
            Some(Self::new(ids))
        }
    }

    /// Fetches keys from another URL (tests).
    pub fn with_jwks_url(mut self, url: impl Into<String>) -> Self {
        self.jwks_url = url.into();
        self
    }

    async fn fetch(&self) -> Result<KeyCache, GoogleError> {
        let resp = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| GoogleError::Unavailable(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Unavailable(format!(
                "jwks status {}",
                resp.status()
            )));
        }
        let ttl = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_max_age)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TTL)
            .min(MAX_TTL);
        let jwks: Jwks = resp
            .json()
            .await
            .map_err(|e| GoogleError::Unavailable(format!("unreadable jwks: {e}")))?;
        let keys = jwks
            .keys
            .into_iter()
            .filter(|k| k.kty == "RSA")
            .filter_map(|k| {
                let (kid, n, e) = (k.kid?, k.n?, k.e?);
                DecodingKey::from_rsa_components(&n, &e)
                    .ok()
                    .map(|key| (kid, key))
            })
            .collect();
        let now = Instant::now();
        Ok(KeyCache {
            keys,
            fetched_at: now,
            expires_at: now + ttl,
        })
    }

    /// The decoding key for `kid`, refreshing the cache when stale or when the kid is unknown.
    async fn key_for(&self, kid: &str) -> Result<DecodingKey, GoogleError> {
        let mut guard = self.cache.lock().await;
        let now = Instant::now();
        let stale = guard.as_ref().is_none_or(|c| c.expires_at <= now);
        if let Some(c) = guard.as_ref()
            && !stale
            && let Some((_, k)) = c.keys.iter().find(|(id, _)| id == kid)
        {
            return Ok(k.clone());
        }
        let may_refetch = stale
            || guard
                .as_ref()
                .is_some_and(|c| now.duration_since(c.fetched_at) >= MIN_REFETCH);
        if may_refetch {
            // An expired cache is never used to validate tokens: an outage surfaces as such.
            *guard = Some(self.fetch().await?);
        }
        guard
            .as_ref()
            .and_then(|c| c.keys.iter().find(|(id, _)| id == kid))
            .map(|(_, k)| k.clone())
            .ok_or_else(|| GoogleError::InvalidToken("unknown signing key".to_owned()))
    }

    pub async fn verify(&self, id_token: &str) -> Result<GoogleIdentity, GoogleError> {
        let bad = |m: &str| GoogleError::InvalidToken(m.to_owned());
        let header = decode_header(id_token).map_err(|_| bad("malformed token"))?;
        if header.alg != Algorithm::RS256 {
            return Err(bad("unsupported algorithm"));
        }
        let kid = header.kid.ok_or_else(|| bad("missing key id"))?;
        let key = self.key_for(&kid).await?;

        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&self.client_ids);
        v.set_issuer(&ISSUERS);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.validate_exp = true;
        let data = decode::<Claims>(id_token, &key, &v).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind as K;
            bad(match e.kind() {
                K::ExpiredSignature => "token expired",
                K::InvalidAudience => "audience mismatch",
                K::InvalidIssuer => "issuer mismatch",
                K::InvalidSignature => "bad signature",
                _ => "token rejected",
            })
        })?;
        let c = data.claims;
        let email = c
            .email
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty())
            .ok_or_else(|| bad("email missing"))?;
        if !c.email_verified {
            return Err(bad("email not verified"));
        }
        if c.sub.trim().is_empty() {
            return Err(bad("subject missing"));
        }
        Ok(GoogleIdentity {
            sub: c.sub,
            email,
            name: c.name,
            picture: c.picture,
        })
    }
}

fn parse_max_age(cache_control: &str) -> Option<u64> {
    cache_control.split(',').find_map(|d| {
        d.trim()
            .strip_prefix("max-age=")
            .and_then(|v| v.trim().trim_matches('"').parse::<u64>().ok())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PEM: &str = include_str!("../tests/test_rsa_key.pem");
    const N: &str = include_str!("../tests/test_rsa_key.n");
    const PEM_OTHER: &str = include_str!("../tests/test_rsa_key_other.pem");
    const N_OTHER: &str = include_str!("../tests/test_rsa_key_other.n");
    const CLIENT: &str = "web-client.apps.example.test";

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn token(pem: &str, kid: &str, mutate: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut claims = json!({
            "iss": "https://accounts.google.com",
            "aud": CLIENT,
            "sub": "1234567890",
            "email": "Ann@Example.test",
            "email_verified": true,
            "name": "Ann",
            "picture": "https://img.test/a.png",
            "iat": now(),
            "exp": now() + 600,
        });
        mutate(&mut claims);
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some(kid.to_owned());
        jsonwebtoken::encode(
            &h,
            &claims,
            &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn jwks(keys: &[(&str, &str)]) -> serde_json::Value {
        json!({"keys": keys.iter().map(|(kid, n)| json!({
            "kid": kid, "kty": "RSA", "alg": "RS256", "use": "sig", "n": n.trim(), "e": "AQAB"
        })).collect::<Vec<_>>()})
    }

    async fn setup(cache_control: &str) -> (MockServer, GoogleVerifier) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", cache_control)
                    .set_body_json(jwks(&[("k1", N)])),
            )
            .mount(&server)
            .await;
        let v = GoogleVerifier::new(vec!["other-client".into(), CLIENT.into()])
            .unwrap()
            .with_jwks_url(format!("{}/certs", server.uri()));
        (server, v)
    }

    #[tokio::test]
    async fn accepts_valid_token_and_caches_keys() {
        let (server, v) = setup("public, max-age=3600, must-revalidate").await;
        let id = v.verify(&token(PEM, "k1", |_| {})).await.unwrap();
        assert_eq!(id.sub, "1234567890");
        assert_eq!(id.email, "Ann@Example.test");
        assert_eq!(id.name.as_deref(), Some("Ann"));
        v.verify(&token(PEM, "k1", |c| {
            c["iss"] = json!("accounts.google.com")
        }))
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn max_age_zero_refetches() {
        let (server, v) = setup("max-age=0").await;
        v.verify(&token(PEM, "k1", |_| {})).await.unwrap();
        v.verify(&token(PEM, "k1", |_| {})).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn string_email_verified_accepted() {
        let (_s, v) = setup("max-age=60").await;
        v.verify(&token(PEM, "k1", |c| c["email_verified"] = json!("true")))
            .await
            .unwrap();
    }

    async fn rejected(v: &GoogleVerifier, t: String, needle: &str) {
        match v.verify(&t).await {
            Err(GoogleError::InvalidToken(m)) => {
                assert!(m.contains(needle), "{m} should contain {needle}")
            }
            other => panic!("expected InvalidToken({needle}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_bad_claims() {
        let (_s, v) = setup("max-age=3600").await;
        rejected(
            &v,
            token(PEM, "k1", |c| c["aud"] = json!("evil")),
            "audience",
        )
        .await;
        rejected(
            &v,
            token(PEM, "k1", |c| c["iss"] = json!("https://evil.test")),
            "issuer",
        )
        .await;
        rejected(
            &v,
            token(PEM, "k1", |c| c["exp"] = json!(now() - 3600)),
            "expired",
        )
        .await;
        rejected(
            &v,
            token(PEM, "k1", |c| c["email_verified"] = json!(false)),
            "not verified",
        )
        .await;
        rejected(
            &v,
            token(PEM, "k1", |c| {
                c.as_object_mut().unwrap().remove("email");
            }),
            "email missing",
        )
        .await;
        rejected(&v, "not-a-jwt".into(), "malformed").await;
    }

    #[tokio::test]
    async fn rejects_foreign_signature_and_unknown_kid() {
        let (_s, v) = setup("max-age=3600").await;
        rejected(&v, token(PEM_OTHER, "k1", |_| {}), "signature").await;
        rejected(&v, token(PEM, "nope", |_| {}), "unknown signing key").await;
    }

    #[tokio::test]
    async fn rejects_hmac_algorithm_confusion() {
        let (_s, v) = setup("max-age=3600").await;
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some("k1".into());
        let t = jsonwebtoken::encode(
            &h,
            &json!({"iss": "accounts.google.com", "aud": CLIENT, "sub": "1", "exp": now() + 60,
                    "email": "a@b.test", "email_verified": true}),
            &EncodingKey::from_secret(N.trim().as_bytes()),
        )
        .unwrap();
        rejected(&v, t, "algorithm").await;
    }

    #[tokio::test]
    async fn rotated_key_is_picked_up_after_refetch_window() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "max-age=3600")
                    .set_body_json(jwks(&[("k1", N)])),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let v = GoogleVerifier::new(vec![CLIENT.into()])
            .unwrap()
            .with_jwks_url(format!("{}/certs", server.uri()));
        v.verify(&token(PEM, "k1", |_| {})).await.unwrap();
        // Unknown kid right after a fetch does not hammer Google.
        rejected(&v, token(PEM_OTHER, "k2", |_| {}), "unknown signing key").await;
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        // Pretend the last fetch was long ago.
        v.cache.lock().await.as_mut().unwrap().fetched_at = Instant::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "max-age=3600")
                    .set_body_json(jwks(&[("k1", N), ("k2", N_OTHER)])),
            )
            .mount(&server)
            .await;
        v.verify(&token(PEM_OTHER, "k2", |_| {})).await.unwrap();
    }

    #[tokio::test]
    async fn jwks_outage_is_unavailable_not_invalid() {
        let v = GoogleVerifier::new(vec![CLIENT.into()])
            .unwrap()
            .with_jwks_url("http://127.0.0.1:1/certs");
        assert!(matches!(
            v.verify(&token(PEM, "k1", |_| {})).await,
            Err(GoogleError::Unavailable(_))
        ));
    }

    #[test]
    fn client_id_parsing() {
        assert!(GoogleVerifier::from_client_ids_csv(" , ").is_none());
        assert!(GoogleVerifier::from_client_ids_csv("a, b").unwrap().is_ok());
        assert_eq!(
            parse_max_age("public, max-age=19234, must-revalidate"),
            Some(19234)
        );
        assert_eq!(parse_max_age("no-store"), None);
    }
}
