use std::sync::Arc;

use async_trait::async_trait;
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
use warptalk_contracts::streams::{EMBEDDING_INDEX_REQUESTS, EMBEDDING_INDEX_RESULTS};

use crate::application::{IndexingService, JobDisposition};
use crate::domain::{IndexRequest, PurgeRequest};

/// Stream of purge requests (`knowledge:purge_requests`).
pub const KNOWLEDGE_PURGE_REQUESTS: &str = "knowledge:purge_requests";
/// Stream results are published to.
pub const INDEX_RESULTS_STREAM: &str = EMBEDDING_INDEX_RESULTS;

pub const INDEX_GROUP: &str = "embedding-workers";
pub const PURGE_GROUP: &str = "knowledge-purge-workers";

/// Consumer config for `embedding:index_requests`. The handler owns attempt counting and
/// dead-lettering (job table); the bus limit is only a backstop above it.
pub fn index_consumer_config() -> ConsumerConfig {
    let mut c = ConsumerConfig::new(EMBEDDING_INDEX_REQUESTS, INDEX_GROUP);
    c.max_deliveries = 25;
    c.claim_idle_ms = 15_000;
    c
}

pub fn purge_consumer_config() -> ConsumerConfig {
    ConsumerConfig::new(KNOWLEDGE_PURGE_REQUESTS, PURGE_GROUP)
}

fn map(d: JobDisposition) -> Result<(), HandlerError> {
    match d {
        JobDisposition::Done => Ok(()),
        JobDisposition::Retry(m) => Err(HandlerError::Retry(m)),
        JobDisposition::Dead(m) => Err(HandlerError::Fatal(m)),
    }
}

/// Handler for `embedding:index_requests`.
pub struct IndexRequestHandler {
    svc: Arc<IndexingService>,
}

impl IndexRequestHandler {
    pub fn new(svc: Arc<IndexingService>) -> Self {
        Self { svc }
    }
}

#[async_trait]
impl StreamHandler for IndexRequestHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let req = IndexRequest::from_fields(&msg.fields)
            .map_err(|e| HandlerError::Fatal(format!("undecodable index request: {e}")))?;
        map(self.svc.handle(&req, &msg.id).await)
    }
}

/// Handler for `knowledge:purge_requests`.
pub struct PurgeRequestHandler {
    svc: Arc<IndexingService>,
}

impl PurgeRequestHandler {
    pub fn new(svc: Arc<IndexingService>) -> Self {
        Self { svc }
    }
}

#[async_trait]
impl StreamHandler for PurgeRequestHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let req = PurgeRequest::from_fields(&msg.fields)
            .map_err(|e| HandlerError::Fatal(format!("undecodable purge request: {e}")))?;
        map(self.svc.handle_purge(&req).await)
    }
}
