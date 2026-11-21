#![allow(clippy::unwrap_used)]
//! Redis cache against the local Redis. Skips when `REDIS_URL` is unset.

use std::time::Duration;

use warptalk_tts::{CachedAudio, GenConfig, RedisTtsCache, TtsCache, cache_key};

#[tokio::test]
async fn redis_cache_roundtrip_and_ttl() {
    let Ok(url) = std::env::var("REDIS_URL") else {
        println!("REDIS_URL unset: skipping redis cache test");
        return;
    };
    let cache = RedisTtsCache::connect(&url).await.unwrap();
    let key = cache_key(
        &uuid::Uuid::now_v7().to_string(),
        "vi",
        "xin chao",
        "standard",
        "v",
        24_000,
        &GenConfig::default(),
    );
    assert!(cache.get(&key).await.is_none());
    let audio = CachedAudio {
        sample_rate: 24_000,
        pcm: vec![0.0, 0.25, -0.25, 0.5].into(),
    };
    cache.put(&key, &audio, Duration::from_secs(2)).await;
    let got = cache.get(&key).await.unwrap();
    assert_eq!(got.sample_rate, 24_000);
    assert_eq!(got.pcm.len(), 4);
    assert!((got.pcm[3] - 0.5).abs() < 1e-3);
}
