//! Synthesis cache: key construction and the in-memory / Redis implementations.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::ports::{CachedAudio, GenConfig, TtsCache};

/// `tts:cache:{sha256}` over (speaker, language, normalised text, voice mode and id, sample
/// rate, delivery). Delivery is part of the key: same words, same voice, said differently is
/// DIFFERENT audio, so catch-up or a shouted "okay" never replays a calm render. It is appended
/// only when there ARE delivery controls.
pub fn cache_key(
    speaker: &str,
    target_lang: &str,
    text: &str,
    voice_mode: &str,
    voice_id: &str,
    sample_rate: u32,
    cfg: &GenConfig,
) -> String {
    let normalized = text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let mut material =
        format!("{speaker}|{target_lang}|{normalized}|{voice_mode}:{voice_id}|sr={sample_rate}");
    if !cfg.is_empty() {
        let mut c = cfg.clone();
        c.speed = c.speed.map(|v| (v * 1000.0).round() / 1000.0);
        c.volume = c.volume.map(|v| (v * 1000.0).round() / 1000.0);
        material.push('|');
        material.push_str(&serde_json::to_string(&c).unwrap_or_default());
    }
    let digest = Sha256::digest(material.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("tts:cache:{hex}")
}

type Store = HashMap<String, (Instant, Vec<u8>)>;

pub struct InMemoryTtsCache {
    max_entries: usize,
    store: Mutex<Store>,
}

impl InMemoryTtsCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            store: Mutex::new(HashMap::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.store.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InMemoryTtsCache {
    fn default() -> Self {
        Self::new(2048)
    }
}

#[async_trait]
impl TtsCache for InMemoryTtsCache {
    async fn get(&self, key: &str) -> Option<CachedAudio> {
        let mut s = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        match s.get(key) {
            Some((exp, b)) if *exp > Instant::now() => CachedAudio::decode(b),
            Some(_) => {
                s.remove(key);
                None
            }
            None => None,
        }
    }

    async fn put(&self, key: &str, audio: &CachedAudio, ttl: Duration) {
        let mut s = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if s.len() >= self.max_entries {
            s.retain(|_, (exp, _)| *exp > now);
            if s.len() >= self.max_entries
                && let Some(k) = s.iter().min_by_key(|(_, (e, _))| *e).map(|(k, _)| k.clone())
            {
                s.remove(&k);
            }
        }
        s.insert(key.to_string(), (now + ttl, audio.encode()));
    }
}

/// Redis implementation; the key from [`cache_key`] is used as is. Errors are logged and
/// swallowed (a cache outage is a miss).
pub struct RedisTtsCache {
    conn: redis::aio::ConnectionManager,
}

impl RedisTtsCache {
    pub fn new(conn: redis::aio::ConnectionManager) -> Self {
        Self { conn }
    }

    pub async fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        Ok(Self::new(client.get_connection_manager().await?))
    }
}

#[async_trait]
impl TtsCache for RedisTtsCache {
    async fn get(&self, key: &str) -> Option<CachedAudio> {
        let mut c = self.conn.clone();
        match redis::cmd("GET").arg(key).query_async::<Option<Vec<u8>>>(&mut c).await {
            Ok(v) => v.and_then(|b| CachedAudio::decode(&b)),
            Err(e) => {
                warn!(error = %e, "tts cache get failed");
                None
            }
        }
    }

    async fn put(&self, key: &str, audio: &CachedAudio, ttl: Duration) {
        let mut c = self.conn.clone();
        let r = redis::cmd("SET")
            .arg(key)
            .arg(audio.encode())
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async::<()>(&mut c)
            .await;
        if let Err(e) = r {
            warn!(error = %e, "tts cache put failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_normalised_and_delivery_sensitive() {
        let g = GenConfig::default();
        let a = cache_key("s", "vi", "Hello  World", "standard", "v1", 44100, &g);
        assert!(a.starts_with("tts:cache:") && a.len() == "tts:cache:".len() + 64);
        assert_eq!(a, cache_key("s", "vi", "hello world", "standard", "v1", 44100, &g));
        assert_ne!(a, cache_key("s", "vi", "hello world", "cloned", "v1", 44100, &g));
        assert_ne!(a, cache_key("s", "vi", "hello world", "standard", "v2", 44100, &g));
        assert_ne!(a, cache_key("s", "vi", "hello world", "standard", "v1", 24000, &g));
        let fast = GenConfig {
            speed: Some(1.2),
            ..GenConfig::default()
        };
        assert_ne!(a, cache_key("s", "vi", "hello world", "standard", "v1", 44100, &fast));
    }

    #[test]
    fn audio_roundtrip_and_empty_is_miss() {
        let a = CachedAudio {
            sample_rate: 24000,
            pcm: vec![0.0, 0.5, -0.5].into(),
        };
        let d = CachedAudio::decode(&a.encode()).unwrap_or_else(|| unreachable!());
        assert_eq!(d.sample_rate, 24000);
        assert!((d.pcm[1] - 0.5).abs() < 1e-3);
        assert!(CachedAudio::decode(&24000u32.to_le_bytes()).is_none());
        assert!(CachedAudio::decode(&[1]).is_none());
    }
}
