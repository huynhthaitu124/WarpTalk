//! Async plane: Redis Streams consumers (transcript persistence now; billing metering,
//! embeddings, summaries, suggestions, notifications later).

pub mod adapters;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use warptalk_bus::{ConsumerConfig, StreamHandler, run_consumer};
use warptalk_config::JwtConfig;
use warptalk_kernel::SystemClock;
use warptalk_transcript::application::ports::{AlwaysSaveRoomPolicy, NoopReadAccess, NoopRoomHost};
use warptalk_transcript::{TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;

use crate::adapters::{PgRoomDirectory, PgWorkspaceAiPolicy};

/// How long shutdown waits for consumers to stop before giving up.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Build the transcript module with the worker's simple adapters.
pub async fn build_transcript_module(
    pool: PgPool,
    redis_url: &str,
) -> anyhow::Result<TranscriptModule> {
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    // The worker mounts no HTTP routes; the module only needs keys to be constructible.
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
        issuer: "warptalk-worker".into(),
        audience: "warptalk-worker".into(),
        access_ttl_seconds: 1,
        refresh_ttl_seconds: 1,
    });
    Ok(TranscriptModule::new(TranscriptDeps {
        pool: pool.clone(),
        redis,
        jwt,
        clock: Arc::new(SystemClock),
        rooms: Arc::new(PgRoomDirectory::new(pool.clone())),
        // TODO: replace with the room module's SaveTranscript setting (spec 12 §5.4, WT-587).
        room_policy: Arc::new(AlwaysSaveRoomPolicy),
        workspace_ai: Arc::new(PgWorkspaceAiPolicy::new(pool)),
        host: Arc::new(NoopRoomHost),
        read_access: Arc::new(NoopReadAccess),
    }))
}

/// Spawn one `run_consumer` task per consumer, each on its own Redis connection
/// (XREADGROUP BLOCK must not share a multiplexed connection with other commands).
pub async fn spawn_consumers(
    redis_url: &str,
    consumers: Vec<(ConsumerConfig, Arc<dyn StreamHandler>)>,
    shutdown: &watch::Receiver<bool>,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::with_capacity(consumers.len());
    for (cfg, handler) in consumers {
        let conn = warptalk_bus::connect(redis_url)
            .await
            .context("connecting consumer to Redis")?;
        handles.push(tokio::spawn(run_consumer(
            conn,
            cfg,
            handler,
            shutdown.clone(),
        )));
    }
    Ok(handles)
}

/// Wait for all consumer tasks, bounded by the shutdown grace period.
pub async fn join_consumers(handles: Vec<JoinHandle<()>>) {
    let all = async {
        for h in handles {
            if let Err(e) = h.await {
                tracing::error!(error = %e, "consumer task failed");
            }
        }
    };
    if tokio::time::timeout(SHUTDOWN_GRACE, all).await.is_err() {
        tracing::warn!("consumers did not stop within the grace period");
    }
}
