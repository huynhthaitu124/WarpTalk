//! HS256 access-token issue/verify.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{AppError, UserId};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Claims {
    pub sub: UserId,
    /// Refresh-token family id.
    pub sid: Uuid,
    /// System admin flag.
    pub sa: bool,
    pub iat: i64,
    pub exp: i64,
    pub iss: String,
    pub aud: String,
}

struct Inner {
    enc: EncodingKey,
    dec: DecodingKey,
    issuer: String,
    audience: String,
    access_ttl: i64,
}

#[derive(Clone)]
pub struct JwtKeys {
    inner: Arc<Inner>,
}

impl JwtKeys {
    pub fn from_config(cfg: &warptalk_config::JwtConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                enc: EncodingKey::from_secret(cfg.secret.as_bytes()),
                dec: DecodingKey::from_secret(cfg.secret.as_bytes()),
                issuer: cfg.issuer.clone(),
                audience: cfg.audience.clone(),
                access_ttl: i64::try_from(cfg.access_ttl_seconds).unwrap_or(i64::MAX / 4),
            }),
        }
    }

    pub fn access_ttl_seconds(&self) -> i64 {
        self.inner.access_ttl
    }

    pub fn issue_access(
        &self,
        user: UserId,
        family: Uuid,
        system_admin: bool,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), AppError> {
        let expires = now + Duration::seconds(self.inner.access_ttl);
        let claims = Claims {
            sub: user,
            sid: family,
            sa: system_admin,
            iat: now.timestamp(),
            exp: expires.timestamp(),
            iss: self.inner.issuer.clone(),
            aud: self.inner.audience.clone(),
        };
        let token = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.inner.enc)
            .map_err(|e| AppError::Internal(format!("jwt encode: {e}")))?;
        Ok((token, expires))
    }

    pub fn verify(&self, token: &str) -> Result<Claims, AppError> {
        let mut v = Validation::new(Algorithm::HS256);
        v.leeway = 30;
        v.validate_exp = true;
        v.set_issuer(&[&self.inner.issuer]);
        v.set_audience(&[&self.inner.audience]);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        jsonwebtoken::decode::<Claims>(token, &self.inner.dec, &v)
            .map(|d| d.claims)
            .map_err(|_| AppError::Unauthorized("invalid or expired token".to_owned()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use warptalk_config::JwtConfig;

    pub(crate) fn cfg(secret: &str, aud: &str) -> JwtConfig {
        JwtConfig {
            secret: secret.to_owned(),
            issuer: "iss".to_owned(),
            audience: aud.to_owned(),
            access_ttl_seconds: 900,
            refresh_ttl_seconds: 1000,
        }
    }

    const S: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn roundtrip() {
        let k = JwtKeys::from_config(&cfg(S, "a"));
        let (u, f) = (UserId::new(), Uuid::now_v7());
        let now = Utc::now();
        let (t, exp) = k.issue_access(u, f, true, now).unwrap();
        assert_eq!(exp.timestamp(), now.timestamp() + 900);
        let c = k.verify(&t).unwrap();
        assert_eq!(c.sub, u);
        assert_eq!(c.sid, f);
        assert!(c.sa);
        assert_eq!(k.access_ttl_seconds(), 900);
    }

    #[test]
    fn rejects_wrong_secret_aud_and_expired() {
        let k = JwtKeys::from_config(&cfg(S, "a"));
        let (t, _) = k
            .issue_access(UserId::new(), Uuid::now_v7(), false, Utc::now())
            .unwrap();
        let other = JwtKeys::from_config(&cfg("fedcba9876543210fedcba9876543210", "a"));
        assert!(matches!(other.verify(&t), Err(AppError::Unauthorized(_))));
        let other = JwtKeys::from_config(&cfg(S, "b"));
        assert!(matches!(other.verify(&t), Err(AppError::Unauthorized(_))));
        let (old, _) = k
            .issue_access(
                UserId::new(),
                Uuid::now_v7(),
                false,
                Utc::now() - Duration::seconds(2000),
            )
            .unwrap();
        assert!(matches!(k.verify(&old), Err(AppError::Unauthorized(_))));
        assert!(k.verify("garbage").is_err());
    }
}
