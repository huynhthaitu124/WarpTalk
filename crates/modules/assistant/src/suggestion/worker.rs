//! `SuggestionWorker`: consumes `stt:results` in its own group and runs the engine without ever
//! blocking the consumer loop (captions are never delayed by a slow model).
//!
//! Per message the cheap front half runs inline; the model stages run on a spawned task. At most
//! one pipeline per room is in flight (a segment arriving meanwhile only feeds the window), and a
//! global semaphore bounds concurrent model pipelines across rooms.

use std::sync::Arc;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage, run_consumer};
use warptalk_contracts::events::{MEETING_END_MARKER, SttResult};
use warptalk_contracts::streams::STT_RESULTS;

use super::engine::{Admission, SuggestionEngine};
use super::model::SUGGESTION_GROUP;

pub struct SuggestionWorker {
    engine: Arc<SuggestionEngine>,
    tasks: Mutex<JoinSet<()>>,
    global: Arc<Semaphore>,
}

impl SuggestionWorker {
    /// `max_concurrent_pipelines`: model pipelines allowed to run at once across all rooms.
    pub fn new(engine: Arc<SuggestionEngine>, max_concurrent_pipelines: usize) -> Self {
        Self {
            engine,
            tasks: Mutex::new(JoinSet::new()),
            global: Arc::new(Semaphore::new(max_concurrent_pipelines.max(1))),
        }
    }

    pub fn engine(&self) -> &Arc<SuggestionEngine> {
        &self.engine
    }

    /// Consumer settings for `stt:results` / group `suggestion-workers`.
    pub fn consumer_config() -> ConsumerConfig {
        ConsumerConfig::new(STT_RESULTS, SUGGESTION_GROUP)
    }

    /// Wait for every spawned pipeline to finish (shutdown, tests).
    pub async fn drain(&self) {
        let mut set = {
            let mut guard = self.tasks.lock().await;
            std::mem::take(&mut *guard)
        };
        while set.join_next().await.is_some() {}
    }

    /// Run the consumer loop until `shutdown` flips, then drain in-flight pipelines.
    pub fn spawn(
        self: Arc<Self>,
        conn: ConnectionManager,
        shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let handler: Arc<dyn StreamHandler> = self.clone();
            run_consumer(conn, Self::consumer_config(), handler, shutdown).await;
            self.drain().await;
        })
    }
}

#[async_trait]
impl StreamHandler for SuggestionWorker {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        // A malformed entry will never parse: dead-letter rather than redeliver.
        let stt: SttResult = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("bad stt:results entry: {e}")))?;
        if stt.text == MEETING_END_MARKER {
            self.engine.forget_room(stt.meeting_id);
            return Ok(());
        }
        let admitted = match self.engine.admit(&stt).await {
            Admission::Rejected(_) => return Ok(()),
            Admission::Admitted(a) => a,
        };
        let engine = self.engine.clone();
        let global = self.global.clone();
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let Ok(_permit) = global.acquire_owned().await else {
                return;
            };
            let outcome = engine.run(*admitted).await;
            tracing::debug!(outcome = outcome.label(), "suggestion pipeline done");
        });
        // Always Ok: any failure inside the pipeline is silence, never a redelivery.
        Ok(())
    }
}
