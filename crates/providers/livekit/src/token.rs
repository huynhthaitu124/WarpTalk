//! LiveKit access tokens (HS256 JWT with a `video` grant).

use std::time::Duration;

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::{LiveKitConfig, LiveKitError};

/// LiveKit `video` grant (camelCase on the wire, unset options omitted).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoGrant {
    #[serde(default, skip_serializing_if = "is_false")]
    pub room_join: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default)]
    pub can_publish: bool,
    #[serde(default)]
    pub can_subscribe: bool,
    #[serde(default)]
    pub can_publish_data: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_publish_sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_update_own_metadata: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_create: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_record: Option<bool>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Full JWT claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenClaims {
    pub iss: String,
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub nbf: i64,
    pub exp: i64,
    pub video: VideoGrant,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// Builder for LiveKit access tokens.
#[derive(Debug, Clone)]
pub struct AccessTokenBuilder {
    api_key: String,
    api_secret: String,
    identity: String,
    name: Option<String>,
    metadata: Option<String>,
    ttl: Duration,
    grant: VideoGrant,
}

impl AccessTokenBuilder {
    pub fn new(cfg: &LiveKitConfig, identity: impl Into<String>) -> Self {
        Self {
            api_key: cfg.api_key.clone(),
            api_secret: cfg.api_secret.clone(),
            identity: identity.into(),
            name: None,
            metadata: None,
            ttl: Duration::from_secs(4 * 3600),
            grant: VideoGrant::default(),
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn room(mut self, room: impl Into<String>) -> Self {
        self.grant.room = Some(room.into());
        self
    }

    pub fn room_join(mut self, v: bool) -> Self {
        self.grant.room_join = v;
        self
    }

    pub fn can_publish(mut self, v: bool) -> Self {
        self.grant.can_publish = v;
        self
    }

    pub fn can_subscribe(mut self, v: bool) -> Self {
        self.grant.can_subscribe = v;
        self
    }

    pub fn can_publish_data(mut self, v: bool) -> Self {
        self.grant.can_publish_data = v;
        self
    }

    pub fn can_publish_sources(mut self, sources: Vec<String>) -> Self {
        self.grant.can_publish_sources = Some(sources);
        self
    }

    pub fn can_update_own_metadata(mut self, v: bool) -> Self {
        self.grant.can_update_own_metadata = Some(v);
        self
    }

    pub fn hidden(mut self, v: bool) -> Self {
        self.grant.hidden = Some(v);
        self
    }

    pub fn room_admin(mut self, v: bool) -> Self {
        self.grant.room_admin = Some(v);
        self
    }

    pub fn room_create(mut self, v: bool) -> Self {
        self.grant.room_create = Some(v);
        self
    }

    pub fn room_record(mut self, v: bool) -> Self {
        self.grant.room_record = Some(v);
        self
    }

    /// Claims as they will be signed at `now`.
    pub fn claims(&self, now: DateTime<Utc>) -> TokenClaims {
        let ttl = i64::try_from(self.ttl.as_secs()).unwrap_or(i64::MAX / 4);
        TokenClaims {
            iss: self.api_key.clone(),
            sub: self.identity.clone(),
            name: self.name.clone(),
            nbf: now.timestamp(),
            exp: now.timestamp().saturating_add(ttl),
            video: self.grant.clone(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn build_at(&self, now: DateTime<Utc>) -> Result<String, LiveKitError> {
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &self.claims(now),
            &EncodingKey::from_secret(self.api_secret.as_bytes()),
        )
        .map_err(|e| LiveKitError::Token(e.to_string()))
    }

    pub fn build(&self) -> Result<String, LiveKitError> {
        self.build_at(Utc::now())
    }
}

/// Regular participant: join, publish, subscribe, data channel, update own metadata.
pub fn participant_token(
    cfg: &LiveKitConfig,
    room: &str,
    identity: &str,
    name: &str,
    ttl: Duration,
    metadata: Option<&str>,
) -> Result<String, LiveKitError> {
    let mut b = AccessTokenBuilder::new(cfg, identity)
        .name(name)
        .ttl(ttl)
        .room(room)
        .room_join(true)
        .can_publish(true)
        .can_subscribe(true)
        .can_publish_data(true)
        .can_update_own_metadata(true);
    if let Some(m) = metadata {
        b = b.metadata(m);
    }
    b.build()
}

/// External-bridge stand-in seat: publish only, cannot subscribe (spec: 11 §3.1 `bridge-token`).
pub fn bridge_token(
    cfg: &LiveKitConfig,
    room: &str,
    identity: &str,
    name: &str,
    ttl: Duration,
) -> Result<String, LiveKitError> {
    AccessTokenBuilder::new(cfg, identity)
        .name(name)
        .ttl(ttl)
        .room(room)
        .room_join(true)
        .can_publish(true)
        .can_subscribe(false)
        .can_publish_data(false)
        .can_publish_sources(vec!["MICROPHONE".to_owned()])
        .build()
}

/// Server-API token (10 min, spec: 11 §5.3). Empty `room` yields a project-wide admin token.
pub fn admin_token(cfg: &LiveKitConfig, room: &str) -> Result<String, LiveKitError> {
    let mut b = AccessTokenBuilder::new(cfg, "warptalk-server")
        .ttl(Duration::from_secs(600))
        .room_admin(true)
        .room_create(true)
        .room_record(true);
    if !room.is_empty() {
        b = b.room(room);
    }
    b.build()
}

/// Hidden pipeline agent: subscribes to everything and may publish (translated audio).
pub fn pipeline_agent_token(
    cfg: &LiveKitConfig,
    room: &str,
    identity: &str,
    ttl: Duration,
) -> Result<String, LiveKitError> {
    AccessTokenBuilder::new(cfg, identity)
        .name(identity)
        .ttl(ttl)
        .room(room)
        .room_join(true)
        .can_publish(true)
        .can_subscribe(true)
        .can_publish_data(true)
        .hidden(true)
        .build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, Validation};

    pub(crate) fn cfg() -> LiveKitConfig {
        LiveKitConfig {
            url: "wss://lk.test".into(),
            api_key: "APIkey".into(),
            api_secret: "secret-secret-secret-secret-1234".into(),
        }
    }

    fn decode(token: &str) -> TokenClaims {
        let mut v = Validation::new(Algorithm::HS256);
        v.set_issuer(&["APIkey"]);
        v.required_spec_claims.clear();
        v.validate_nbf = true;
        jsonwebtoken::decode::<TokenClaims>(
            token,
            &DecodingKey::from_secret(cfg().api_secret.as_bytes()),
            &v,
        )
        .unwrap()
        .claims
    }

    #[test]
    fn participant_claims() {
        let t = participant_token(
            &cfg(),
            "room-1",
            "user-1",
            "Ann",
            Duration::from_secs(4 * 3600),
            Some("{\"a\":1}"),
        )
        .unwrap();
        let c = decode(&t);
        assert_eq!(c.iss, "APIkey");
        assert_eq!(c.sub, "user-1");
        assert_eq!(c.name.as_deref(), Some("Ann"));
        assert_eq!(c.exp - c.nbf, 4 * 3600);
        assert!(c.video.room_join && c.video.can_publish && c.video.can_subscribe);
        assert!(c.video.can_publish_data);
        assert_eq!(c.video.room.as_deref(), Some("room-1"));
        assert_eq!(c.metadata.as_deref(), Some("{\"a\":1}"));
        assert_eq!(c.video.hidden, None);
    }

    #[test]
    fn wire_names_are_camel_case() {
        let t = participant_token(&cfg(), "r", "u", "n", Duration::from_secs(60), None).unwrap();
        let payload = t.split('.').nth(1).unwrap();
        let raw =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
                .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["video"]["roomJoin"], true);
        assert_eq!(v["video"]["canPublishData"], true);
        assert!(v["video"].get("hidden").is_none());
    }

    #[test]
    fn bridge_token_cannot_subscribe() {
        let c = decode(
            &bridge_token(&cfg(), "r", "bridge", "Bridge", Duration::from_secs(60)).unwrap(),
        );
        assert!(c.video.can_publish);
        assert!(!c.video.can_subscribe);
        assert!(c.video.room_join);
    }

    #[test]
    fn admin_and_agent_presets() {
        let a = decode(&admin_token(&cfg(), "r").unwrap());
        assert_eq!(a.video.room_admin, Some(true));
        assert!(!a.video.room_join);
        assert_eq!(a.exp - a.nbf, 600);
        let p =
            decode(&pipeline_agent_token(&cfg(), "r", "agent", Duration::from_secs(60)).unwrap());
        assert_eq!(p.video.hidden, Some(true));
        assert!(p.video.can_subscribe && p.video.can_publish);
    }
}
