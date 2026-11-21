use std::sync::Mutex;

use async_trait::async_trait;
use redis::aio::ConnectionManager;

use crate::application::ports::ResultPublisher;
use crate::domain::IndexResult;

const RESULTS_MAXLEN: usize = 10_000;

/// XADDs results to a Redis stream (flat upstream fields + `data` JSON).
pub struct RedisResultPublisher {
    conn: ConnectionManager,
    stream: String,
}

impl RedisResultPublisher {
    pub fn new(conn: ConnectionManager, stream: impl Into<String>) -> Self {
        Self {
            conn,
            stream: stream.into(),
        }
    }
}

#[async_trait]
impl ResultPublisher for RedisResultPublisher {
    async fn publish(&self, result: &IndexResult) -> Result<(), String> {
        let mut conn = self.conn.clone();
        warptalk_bus::xadd(&mut conn, &self.stream, &result.to_fields(), RESULTS_MAXLEN)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Collects results in memory (tests, local runs).
#[derive(Default)]
pub struct MemoryResultPublisher {
    results: Mutex<Vec<IndexResult>>,
}

impl MemoryResultPublisher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn results(&self) -> Vec<IndexResult> {
        self.results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl ResultPublisher for MemoryResultPublisher {
    async fn publish(&self, result: &IndexResult) -> Result<(), String> {
        self.results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(result.clone());
        Ok(())
    }
}
