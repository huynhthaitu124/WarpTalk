//! Usage metering: price a `usage:events` entry via the rate card, record it and debit the
//! wallet once per `event_id` (NFR-RL-04). BYO usage is recorded with 0 credits.

use std::sync::Arc;

use warptalk_kernel::{AppResult, Clock, WorkspaceId};

use super::ports::{BillingStore, ServiceStateSink, UsageOutcome};
use crate::domain::{ServiceState, UsageEvent, price_event};

pub struct MeteringService {
    store: Arc<dyn BillingStore>,
    sink: Arc<dyn ServiceStateSink>,
    clock: Arc<dyn Clock>,
}

impl MeteringService {
    pub fn new(
        store: Arc<dyn BillingStore>,
        sink: Arc<dyn ServiceStateSink>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { store, sink, clock }
    }

    /// Idempotent: a replayed `event_id` changes nothing and reports `duplicate`.
    pub async fn process(&self, e: &UsageEvent) -> AppResult<UsageOutcome> {
        let card = self.store.rate_card().await?;
        let priced = price_event(&card, e);
        if priced.unpriced {
            tracing::warn!(
                feature = e.feature.as_str(), model = %e.model,
                "no rate-card entry: usage recorded with 0 credits"
            );
        }
        let out = self.store.record_usage(e, &priced, self.clock.now()).await?;
        // Usage can only worsen the state; recovery is published by grants and adjustments.
        if !out.duplicate && out.state != ServiceState::Healthy {
            self.sink
                .publish(WorkspaceId::from_uuid(e.workspace_id), &out.state)
                .await;
        }
        Ok(out)
    }
}
