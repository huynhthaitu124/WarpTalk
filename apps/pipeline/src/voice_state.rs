//! Runtime voice consent for the TTS stage (spec 13 section 5), read from Redis (the pipeline
//! has no database).
//!
//! The api writes `voice:consent:{userId}` on every consent / enrollment change:
//! `{"consent": bool, "voiceId": string?}`. A missing key means no consent. A pipeline speaker is
//! a participant: the downstream stage resolves it to the user id (from the routes snapshot's
//! `source_user_id`) before asking, so the `speaker` argument here is always a user id.
//!
//! Lookups are cached in process for a short TTL (default 2 s) so a revoke takes effect within
//! seconds without a Redis round trip per sentence. Transport errors are NOT cached and fail
//! closed (`ConsentChecker` returns `Err`, which the voice resolver treats as "no consent").

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;
use warptalk_tts::{ConsentChecker, TtsError, VoiceProfiles};

use crate::glossary::KvSource;

pub fn consent_key(speaker: &str) -> String {
    format!("voice:consent:{speaker}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceState {
    #[serde(default)]
    pub consent: bool,
    #[serde(default)]
    pub voice_id: Option<String>,
}

const MAX_ENTRIES: usize = 4_096;

pub struct RedisVoiceState {
    kv: Arc<dyn KvSource>,
    ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, VoiceState)>>,
}

impl RedisVoiceState {
    pub fn new(kv: Arc<dyn KvSource>) -> Arc<Self> {
        Self::with_ttl(kv, Duration::from_secs(2))
    }

    pub fn with_ttl(kv: Arc<dyn KvSource>, ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            kv,
            ttl,
            cache: Mutex::new(HashMap::new()),
        })
    }

    async fn state(&self, speaker: &str) -> Result<VoiceState, String> {
        if let Some((at, st)) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(speaker)
            && at.elapsed() < self.ttl
        {
            return Ok(st.clone());
        }
        let raw = self.kv.get(&consent_key(speaker)).await?;
        let st = match raw {
            None => VoiceState::default(),
            // A value we cannot read is "no consent", never "yes".
            Some(r) => serde_json::from_str(&r).unwrap_or_else(|e| {
                warn!(error = %e, "malformed voice consent state; treating as no consent");
                VoiceState::default()
            }),
        };
        let mut c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if c.len() >= MAX_ENTRIES {
            let ttl = self.ttl;
            c.retain(|_, (at, _)| at.elapsed() < ttl);
            if c.len() >= MAX_ENTRIES {
                c.clear();
            }
        }
        c.insert(speaker.to_owned(), (Instant::now(), st.clone()));
        Ok(st)
    }
}

#[async_trait]
impl ConsentChecker for RedisVoiceState {
    async fn voice_clone_consented(&self, _room: &str, speaker: &str) -> Result<bool, TtsError> {
        self.state(speaker)
            .await
            .map(|s| s.consent)
            .map_err(|e| TtsError::Transient(format!("voice consent lookup failed: {e}")))
    }
}

#[async_trait]
impl VoiceProfiles for RedisVoiceState {
    /// The user's enrolled clone; only with active consent.
    async fn carried_clone(&self, _room: &str, speaker: &str) -> Option<String> {
        match self.state(speaker).await {
            Ok(s) if s.consent => s.voice_id,
            _ => None,
        }
    }

    async fn chosen_dub_voice(&self, _room: &str, _speaker: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeKv {
        data: Mutex<HashMap<String, String>>,
        gets: AtomicUsize,
        down: AtomicBool,
    }

    #[async_trait]
    impl KvSource for FakeKv {
        async fn get(&self, key: &str) -> Result<Option<String>, String> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            if self.down.load(Ordering::SeqCst) {
                return Err("redis down".into());
            }
            Ok(self.data.lock().unwrap().get(key).cloned())
        }
    }

    fn set(kv: &FakeKv, user: &str, v: &str) {
        kv.data.lock().unwrap().insert(consent_key(user), v.into());
    }

    #[tokio::test]
    async fn missing_or_malformed_is_no_consent() {
        let kv = Arc::new(FakeKv::default());
        let st = RedisVoiceState::with_ttl(kv.clone(), Duration::ZERO);
        assert!(!st.voice_clone_consented("r", "u1").await.unwrap());
        set(&kv, "u1", "garbage");
        assert!(!st.voice_clone_consented("r", "u1").await.unwrap());
        assert_eq!(st.carried_clone("r", "u1").await, None);
    }

    #[tokio::test]
    async fn consent_and_voice_follow_the_key_and_revoke_wins() {
        let kv = Arc::new(FakeKv::default());
        let st = RedisVoiceState::with_ttl(kv.clone(), Duration::from_millis(30));
        set(&kv, "u1", r#"{"consent":true,"voiceId":"v-1"}"#);
        assert!(st.voice_clone_consented("r", "u1").await.unwrap());
        assert_eq!(st.carried_clone("r", "u1").await.as_deref(), Some("v-1"));
        // cached within the ttl
        let gets = kv.gets.load(Ordering::SeqCst);
        assert!(st.voice_clone_consented("r", "u1").await.unwrap());
        assert_eq!(kv.gets.load(Ordering::SeqCst), gets);
        // revoke: consent false hides the voice id too, visible after the ttl
        set(&kv, "u1", r#"{"consent":false,"voiceId":"v-1"}"#);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!st.voice_clone_consented("r", "u1").await.unwrap());
        assert_eq!(st.carried_clone("r", "u1").await, None);
        // other users unaffected
        assert!(!st.voice_clone_consented("r", "u2").await.unwrap());
    }

    #[tokio::test]
    async fn transport_errors_fail_closed_and_are_not_cached() {
        let kv = Arc::new(FakeKv::default());
        let st = RedisVoiceState::with_ttl(kv.clone(), Duration::from_secs(60));
        set(&kv, "u1", r#"{"consent":true}"#);
        kv.down.store(true, Ordering::SeqCst);
        assert!(st.voice_clone_consented("r", "u1").await.is_err());
        assert_eq!(st.carried_clone("r", "u1").await, None);
        kv.down.store(false, Ordering::SeqCst);
        assert!(st.voice_clone_consented("r", "u1").await.unwrap());
    }

    #[tokio::test]
    async fn reads_the_real_redis_key_when_available() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            return;
        };
        let conn = warptalk_bus::connect(&url).await.unwrap();
        let user = uuid::Uuid::new_v4().to_string();
        let mut c = conn.clone();
        redis::cmd("SET")
            .arg(consent_key(&user))
            .arg(r#"{"consent":true,"voiceId":"v-real"}"#)
            .query_async::<()>(&mut c)
            .await
            .unwrap();
        let st =
            RedisVoiceState::with_ttl(Arc::new(crate::glossary::RedisKv(conn)), Duration::ZERO);
        assert!(st.voice_clone_consented("r", &user).await.unwrap());
        assert_eq!(
            st.carried_clone("r", &user).await.as_deref(),
            Some("v-real")
        );
        redis::cmd("DEL")
            .arg(consent_key(&user))
            .query_async::<()>(&mut c)
            .await
            .unwrap();
        assert!(!st.voice_clone_consented("r", &user).await.unwrap());
    }
}
