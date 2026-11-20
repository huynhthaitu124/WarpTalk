/// Errors returned by the bus layer.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("decode error: {0}")]
    Decode(String),
}
