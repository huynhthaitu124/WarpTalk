//! Redis Streams consumer groups (XREADGROUP, XAUTOCLAIM, dead letter), pub/sub,
//! idempotency keys, leases/leader election.

mod consumer;
mod error;
mod kv;
mod publisher;
mod pubsub;

pub use consumer::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage, run_consumer};
pub use error::BusError;
pub use kv::{claim_once, release_lease, renew_lease, try_acquire_lease};
pub use publisher::{xadd, xadd_json};
pub use pubsub::{publish_json, subscribe};

use redis::aio::ConnectionManager;

/// Open a auto-reconnecting Redis connection.
pub async fn connect(url: &str) -> Result<ConnectionManager, BusError> {
    let client = redis::Client::open(url)?;
    // Bounded initial connect: an unreachable Redis must fail fast instead of retrying forever
    // (a hung `connect` made integration tests and service start-up block silently).
    let cfg = redis::aio::ConnectionManagerConfig::new()
        .set_connection_timeout(std::time::Duration::from_secs(3))
        .set_number_of_retries(2);
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        ConnectionManager::new_with_config(client, cfg),
    )
    .await
    {
        Ok(r) => Ok(r?),
        Err(_) => Err(BusError::Redis(redis::RedisError::from((
            redis::ErrorKind::IoError,
            "redis connect timed out",
        )))),
    }
}
