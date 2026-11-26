#![allow(clippy::unwrap_used)]
//! Per-user audio socket limit across API replicas (Redis-backed). Skips without REDIS_URL.

use std::sync::Arc;
use std::time::Duration;

use warptalk_copilot::application::audio::{ConnectionLimiter, MemoryConnectionLimiter};
use warptalk_copilot::infrastructure::redis_adapters::RedisConnectionLimiter;
use warptalk_kernel::UserId;

const LEASE: Duration = Duration::from_millis(600);

async fn limiter() -> Option<RedisConnectionLimiter> {
    let url = std::env::var("REDIS_URL").ok()?;
    Some(RedisConnectionLimiter::new(
        warptalk_bus::connect(&url).await.unwrap(),
    ))
}

#[tokio::test]
async fn limit_is_shared_between_replicas_and_released_on_close() {
    let (Some(a), Some(b)) = (limiter().await, limiter().await) else {
        println!("skipping: REDIS_URL not set");
        return;
    };
    let user = UserId::new();
    // Two "replicas" share the same Redis: the third socket is refused wherever it lands.
    assert!(a.acquire(user, "c1", 2, LEASE).await.unwrap().is_some());
    assert!(b.acquire(user, "c2", 2, LEASE).await.unwrap().is_some());
    assert!(a.acquire(user, "c3", 2, LEASE).await.unwrap().is_none());
    assert!(b.acquire(user, "c3", 2, LEASE).await.unwrap().is_none());
    // Another user is unaffected.
    assert!(
        a.acquire(UserId::new(), "x", 2, LEASE)
            .await
            .unwrap()
            .is_some()
    );
    // Closing one frees a slot for either replica; release is idempotent.
    b.release(user, "c1").await.unwrap();
    b.release(user, "c1").await.unwrap();
    assert!(b.acquire(user, "c3", 2, LEASE).await.unwrap().is_some());
    a.release(user, "c2").await.unwrap();
    a.release(user, "c3").await.unwrap();
}

#[tokio::test]
async fn crashed_replica_slots_expire_and_heartbeats_keep_live_ones() {
    let Some(a) = limiter().await else {
        println!("skipping: REDIS_URL not set");
        return;
    };
    let user = UserId::new();
    assert!(a.acquire(user, "dead", 2, LEASE).await.unwrap().is_some());
    assert!(a.acquire(user, "live", 2, LEASE).await.unwrap().is_some());
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        a.heartbeat(user, "live", LEASE).await.unwrap();
    }
    // "dead" never heartbeated: its lease lapsed, so one slot is free again, "live" still counts.
    assert!(a.acquire(user, "n1", 2, LEASE).await.unwrap().is_some());
    assert!(a.acquire(user, "n2", 2, LEASE).await.unwrap().is_none());
    // A heartbeat never resurrects a released slot.
    a.release(user, "n1").await.unwrap();
    a.heartbeat(user, "n1", LEASE).await.unwrap();
    assert!(a.acquire(user, "n3", 2, LEASE).await.unwrap().is_some());
    a.release(user, "live").await.unwrap();
    a.release(user, "n3").await.unwrap();
}

#[tokio::test]
async fn memory_limiter_matches_the_contract() {
    let m = Arc::new(MemoryConnectionLimiter::default());
    let user = UserId::new();
    assert!(m.acquire(user, "a", 1, LEASE).await.unwrap().is_some());
    assert!(m.acquire(user, "b", 1, LEASE).await.unwrap().is_none());
    m.release(user, "a").await.unwrap();
    assert_eq!(m.open_for(user), 0);
    assert!(m.acquire(user, "b", 1, LEASE).await.unwrap().is_some());
}
