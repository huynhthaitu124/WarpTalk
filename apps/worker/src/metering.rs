//! Billing metering consumer: `usage:events` -> rate card -> usage record + wallet debit.
//! Idempotent per `event_id` (NFR-RL-04), so redelivery after a crash is harmless.

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use sqlx::PgPool;
use warptalk_billing::application::metering::MeteringService;
use warptalk_billing::application::ports::WorkspaceAccess;
use warptalk_billing::domain::{USAGE_STREAM, UsageEvent};
use warptalk_billing::infrastructure::redis_state::RedisStateSink;
use warptalk_billing::{BillingDeps, BillingModule};
use warptalk_bus::{ConsumerConfig, HandlerError, StreamHandler, StreamMessage};
use warptalk_kernel::{AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};

pub const METERING_GROUP: &str = "billing-metering";

/// The worker serves no HTTP, so nobody is a member.
struct NoAccess;

#[async_trait]
impl WorkspaceAccess for NoAccess {
    async fn member_role(&self, _: WorkspaceId, _: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(None)
    }
}

pub struct MeteringHandler {
    metering: Arc<MeteringService>,
}

impl MeteringHandler {
    pub fn new(metering: Arc<MeteringService>) -> Self {
        Self { metering }
    }
}

#[async_trait]
impl StreamHandler for MeteringHandler {
    async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
        // A malformed entry can never succeed: dead-letter it instead of retrying.
        let event: UsageEvent = msg
            .json()
            .map_err(|e| HandlerError::Fatal(format!("bad usage event: {e}")))?;
        match self.metering.process(&event).await {
            Ok(out) => {
                tracing::debug!(
                    event_id = %event.event_id, credits = out.credits, duplicate = out.duplicate,
                    "usage metered"
                );
                Ok(())
            }
            Err(e) => Err(HandlerError::Retry(e.to_string())),
        }
    }
}

pub fn metering_consumer_config() -> ConsumerConfig {
    ConsumerConfig::new(USAGE_STREAM, METERING_GROUP)
}

pub async fn metering_consumer(
    pool: PgPool,
    redis_url: &str,
) -> anyhow::Result<(ConsumerConfig, Arc<dyn StreamHandler>)> {
    let redis = warptalk_bus::connect(redis_url)
        .await
        .context("connecting to Redis")?;
    let module = BillingModule::new(BillingDeps {
        pool,
        jwt: crate::dummy_jwt(),
        clock: Arc::new(SystemClock),
        access: Arc::new(NoAccess),
        gateway: None,
        state_sink: Some(Arc::new(RedisStateSink::new(redis))),
        config: Default::default(),
    });
    Ok((
        metering_consumer_config(),
        Arc::new(MeteringHandler::new(module.metering())),
    ))
}
