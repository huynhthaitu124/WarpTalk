//! Webhook verification and event model.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{LiveKitConfig, LiveKitError};

/// Accepts a JSON string or number and yields a string (protobuf JSON renders int64/enums both ways).
fn flex_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

fn flex_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => s.parse().ok(),
        Some(Value::Number(n)) => n.as_i64(),
        _ => None,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WebhookRoom {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sid: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WebhookParticipant {
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub sid: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WebhookTrack {
    #[serde(default)]
    pub sid: String,
    /// `AUDIO` / `VIDEO` / `DATA` (numeric 0/1/2 also accepted).
    #[serde(default, rename = "type", deserialize_with = "flex_string")]
    pub kind: Option<String>,
    #[serde(default)]
    pub muted: bool,
}

impl WebhookTrack {
    /// Normalised media type: `AUDIO`, `VIDEO` or `DATA`; unknown values map to `VIDEO` (the column default).
    pub fn media_type(&self) -> &'static str {
        match self.kind.as_deref() {
            Some("AUDIO" | "0") => "AUDIO",
            Some("DATA" | "2") => "DATA",
            _ => "VIDEO",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub location: String,
    #[serde(default, deserialize_with = "flex_i64")]
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressInfo {
    #[serde(default)]
    pub egress_id: String,
    #[serde(default, deserialize_with = "flex_string")]
    pub status: Option<String>,
    #[serde(default)]
    pub file_results: Option<Vec<FileResult>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvent {
    /// Unique delivery-independent event id (stable across redeliveries).
    #[serde(default)]
    pub id: Option<String>,
    pub event: String,
    #[serde(default)]
    pub room: Option<WebhookRoom>,
    #[serde(default)]
    pub participant: Option<WebhookParticipant>,
    #[serde(default)]
    pub track: Option<WebhookTrack>,
    #[serde(default)]
    pub egress_info: Option<EgressInfo>,
    /// Unix seconds.
    #[serde(default, deserialize_with = "flex_i64")]
    pub created_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct WebhookClaims {
    iss: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    exp: Option<i64>,
    #[serde(default)]
    nbf: Option<i64>,
}

fn body_hash(body: &[u8]) -> String {
    STANDARD.encode(Sha256::digest(body))
}

/// Verify a LiveKit webhook delivery: the `Authorization` JWT (bare or `Bearer `-prefixed) must be
/// signed with the API secret by the API key, and its `sha256` claim must equal the base64 SHA-256
/// of the raw body. Returns the parsed event.
pub fn verify_webhook(
    body: &[u8],
    auth_header: &str,
    cfg: &LiveKitConfig,
) -> Result<WebhookEvent, LiveKitError> {
    let mut token = auth_header.trim();
    if token.len() >= 7 && token[..7].eq_ignore_ascii_case("bearer ") {
        token = token[7..].trim();
    }
    if token.is_empty() {
        return Err(LiveKitError::MissingAuth);
    }
    let mut v = Validation::new(Algorithm::HS256);
    v.set_issuer(&[cfg.api_key.as_str()]);
    v.required_spec_claims.clear();
    v.required_spec_claims.insert("iss".to_owned());
    v.validate_exp = true;
    v.validate_nbf = true;
    v.leeway = 30;
    let claims = jsonwebtoken::decode::<WebhookClaims>(
        token,
        &DecodingKey::from_secret(cfg.api_secret.as_bytes()),
        &v,
    )
    .map_err(|e| LiveKitError::InvalidSignature(e.to_string()))?
    .claims;
    let expected = claims
        .sha256
        .ok_or_else(|| LiveKitError::InvalidSignature("missing sha256 claim".into()))?;
    if expected != body_hash(body) {
        return Err(LiveKitError::BodyHashMismatch);
    }
    serde_json::from_slice(body).map_err(|e| LiveKitError::Malformed(e.to_string()))
}

/// Build the `Authorization` value LiveKit would send for `body` (for tests and local tooling).
pub fn sign_webhook(body: &[u8], cfg: &LiveKitConfig) -> Result<String, LiveKitError> {
    let now = Utc::now().timestamp();
    let claims = WebhookClaims {
        iss: cfg.api_key.clone(),
        sha256: Some(body_hash(body)),
        exp: Some(now + 300),
        nbf: Some(now - 5),
    };
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(cfg.api_secret.as_bytes()),
    )
    .map_err(|e| LiveKitError::Token(e.to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cfg(secret: &str) -> LiveKitConfig {
        LiveKitConfig {
            url: "wss://lk.test".into(),
            api_key: "APIkey".into(),
            api_secret: secret.into(),
        }
    }

    const BODY: &str = r#"{"event":"participant_joined","id":"EV_1","room":{"sid":"RM_1","name":"r1"},"participant":{"sid":"PA_1","identity":"u1"},"createdAt":"1700000000"}"#;

    #[test]
    fn verifies_and_parses() {
        let c = cfg("s3cret-s3cret-s3cret-s3cret-0001");
        let auth = sign_webhook(BODY.as_bytes(), &c).unwrap();
        let ev = verify_webhook(BODY.as_bytes(), &auth, &c).unwrap();
        assert_eq!(ev.event, "participant_joined");
        assert_eq!(ev.room.unwrap().name, "r1");
        assert_eq!(ev.participant.unwrap().identity, "u1");
        assert_eq!(ev.created_at, Some(1_700_000_000));
        let ev = verify_webhook(BODY.as_bytes(), &format!("Bearer {auth}"), &c).unwrap();
        assert_eq!(ev.id.as_deref(), Some("EV_1"));
    }

    #[test]
    fn tampered_body_rejected() {
        let c = cfg("s3cret-s3cret-s3cret-s3cret-0001");
        let auth = sign_webhook(BODY.as_bytes(), &c).unwrap();
        let tampered = BODY.replace("u1", "u2");
        let err = verify_webhook(tampered.as_bytes(), &auth, &c).unwrap_err();
        assert!(matches!(err, LiveKitError::BodyHashMismatch));
        assert!(err.is_auth_failure());
    }

    #[test]
    fn wrong_secret_rejected() {
        let good = cfg("s3cret-s3cret-s3cret-s3cret-0001");
        let bad = cfg("other-other-other-other-other-0002");
        let auth = sign_webhook(BODY.as_bytes(), &bad).unwrap();
        let err = verify_webhook(BODY.as_bytes(), &auth, &good).unwrap_err();
        assert!(matches!(err, LiveKitError::InvalidSignature(_)));
        assert!(matches!(
            verify_webhook(BODY.as_bytes(), "  ", &good).unwrap_err(),
            LiveKitError::MissingAuth
        ));
    }

    #[test]
    fn parses_track_and_egress() {
        let body = br#"{"event":"egress_ended","track":{"sid":"TR_1","type":0},"egressInfo":{"egressId":"EG_1","status":"EGRESS_COMPLETE","fileResults":[{"filename":"a.mp4","location":"s3://x","size":"42"}]}}"#;
        let ev: WebhookEvent = serde_json::from_slice(body).unwrap();
        assert_eq!(ev.track.unwrap().media_type(), "AUDIO");
        let e = ev.egress_info.unwrap();
        assert_eq!(e.egress_id, "EG_1");
        assert_eq!(e.file_results.unwrap()[0].size, Some(42));
    }
}
