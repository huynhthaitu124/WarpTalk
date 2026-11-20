//! Redis Streams consumer groups (XREADGROUP, XAUTOCLAIM, dead letter), pub/sub,
//! idempotency keys, leases/leader election.

mod consumer;
mod error;
mod kv;
mod pubsub;
mod publisher;

pub use consumer::{run_consumer, ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
pub use error::BusError;
pub use kv::{claim_once, release_lease, renew_lease, try_acquire_lease};
pub use publisher::{xadd, xadd_json};
pub use pubsub::{publish_json, subscribe};

use redis::aio::ConnectionManager;

/// Open a auto-reconnecting Redis connection.
pub async fn connect(url: &str) -> Result<ConnectionManager, BusError> {
    let client = redis::Client::open(url)?;
    Ok(ConnectionManager::new(client).await?)
}
