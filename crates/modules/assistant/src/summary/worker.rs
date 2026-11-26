//! `SummaryWorker`: two consumers in group `meeting-summary` plus a sweeper.
//!
//! - `stt:results`: the `__MEETING_END__` marker enqueues the room's automatic job (a durable
//!   `pending` row, idempotent per transcript version) and runs it on a spawned task, so the
//!   consumer never blocks on the model. Every other segment is acked untouched.
//! - `assistant:summary_requests`: explicit regenerate (the row already exists).
//! - Sweeper: re-runs `pending` rows nobody picked up and `running` rows that went silent
//!   (worker restart, lost request, transient failure waiting for its next attempt).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
use warptalk_contracts::events::{MEETING_END_MARKER, SttResult};
use warptalk_contracts::streams::{ASSISTANT_SUMMARY_REQUESTS, STT_RESULTS};
use warptalk_kernel::WorkspaceId;

use super::model::{SUMMARY_GROUP, SummaryRequest};
use super::service::SummaryService;

/// A pending job untouched for this long is picked up by the sweeper.
pub const SWEEP_QUIET: Duration = Duration::from_secs(20);
pub const SWEEP_EVERY: Duration = Duration::from_secs(15);

pub struct SummaryWorker {
    service: Arc<SummaryService>,
    tasks: Mutex<JoinSet<()>>,
    permits: Arc<Semaphore>,
}

impl SummaryWorker {
    /// `max_concurrent`: jobs running at once in this process.
    pub fn new(service: Arc<SummaryService>, max_concurrent: usize) -> Self {
        Self {
            service,
            tasks: Mutex::new(JoinSet::new()),
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    pub fn service(&self) -> &Arc<SummaryService> {
        &self.service
    }

    pub fn marker_consumer_config() -> ConsumerConfig {
        ConsumerConfig::new(STT_RESULTS, SUMMARY_GROUP)
    }

    pub fn request_consumer_config() -> ConsumerConfig {
        ConsumerConfig::new(ASSISTANT_SUMMARY_REQUESTS, SUMMARY_GROUP)
    }

    /// Both consumers, ready for `spawn_consumers`.
    pub fn consumers(self: &Arc<Self>) -> Vec<(ConsumerConfig, Arc<dyn StreamHandler>)> {
        vec![
            (
                Self::marker_consumer_config(),
                Arc::new(MarkerHandler(self.clone())) as Arc<dyn StreamHandler>,
            ),
            (
                Self::request_consumer_config(),
                Arc::new(RequestHandler(self.clone())) as Arc<dyn StreamHandler>,
            ),
        ]
    }

    async fn spawn_job(&self, id: uuid::Uuid) {
        let service = self.service.clone();
        let permits = self.permits.clone();
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            let outcome = service.run(id).await;
            tracing::debug!(summary = %id, ?outcome, "summary job finished");
        });
    }

    /// Wait for spawned jobs (shutdown, tests).
    pub async fn drain(&self) {
        let mut set = {
            let mut g = self.tasks.lock().await;
            std::mem::take(&mut *g)
        };
        while set.join_next().await.is_some() {}
    }

    /// One sweep: start every job nobody works on. Returns how many were started.
    pub async fn sweep(&self, quiet: Duration) -> usize {
        self.sweep_scoped(quiet, None).await
    }

    /// Like [`Self::sweep`] but limited to one workspace (`None` = all).
    pub async fn sweep_scoped(&self, quiet: Duration, workspace: Option<WorkspaceId>) -> usize {
        // Silent `running` rows count as stale per room config; the listing uses the platform
        // default and `claim` re-checks with the room's own value.
        let stale = Duration::from_secs(super::config::SummaryConfig::default().stale_running_secs);
        let jobs = self.service.open_jobs(quiet, stale, workspace).await;
        let n = jobs.len();
        for j in jobs {
            self.spawn_job(j.id).await;
        }
        n
    }

    /// Sweeper loop; ends on shutdown after draining running jobs.
    pub fn spawn_sweeper(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(SWEEP_EVERY) => {}
                    _ = shutdown.changed() => {}
                }
                if *shutdown.borrow() {
                    break;
                }
                self.sweep(SWEEP_QUIET).await;
            }
            self.drain().await;
        })
    }
}

struct MarkerHandler(Arc<SummaryWorker>);

#[async_trait]
impl StreamHandler for MarkerHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        // Cheap pre-check so the ordinary segment flood is not fully parsed.
        if !msg
            .fields
            .get("data")
            .is_some_and(|d| d.contains(MEETING_END_MARKER))
        {
            return Ok(());
        }
        let stt: SttResult = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("bad stt:results entry: {e}")))?;
        if stt.text != MEETING_END_MARKER {
            return Ok(());
        }
        match self.0.service.on_meeting_end(stt.meeting_id).await {
            Ok(Some(job)) => {
                self.0.spawn_job(job.id).await;
                Ok(())
            }
            Ok(None) => Ok(()),
            // Database trouble: redelivered, dead-lettered after max deliveries.
            Err(e) => Err(HandlerError::Retry(e.to_string())),
        }
    }
}

struct RequestHandler(Arc<SummaryWorker>);

#[async_trait]
impl StreamHandler for RequestHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        let req: SummaryRequest = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("bad summary request: {e}")))?;
        self.0.spawn_job(req.summary_id).await;
        Ok(())
    }
}
