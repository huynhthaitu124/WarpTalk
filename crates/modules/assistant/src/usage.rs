//! Usage metering hook: LLM calls publish `usage:events` entries (consumed by the billing
//! metering worker). Wire contract shared with `warptalk-billing` (`domain::UsageEvent`).
//!
//! Boundary: a `WorkspaceControlled` endpoint is the customer's own (BYO) and is recorded with
//! cost 0; `External` is the platform's provider and is charged. (`ResolvedLlm` carries no
//! explicit BYO flag, so a customer key pointing at a third-party endpoint is metered as
//! platform until the resolver exposes one.)

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use serde::Serialize;
use uuid::Uuid;
use warptalk_llm::Usage;

use crate::suggestion::DataBoundary;

/// Redis stream of billable usage.
pub const USAGE_STREAM: &str = "usage:events";
const USAGE_STREAM_MAXLEN: usize = 200_000;

#[derive(Debug, Clone, Serialize)]
pub struct UsageEvent {
    /// Idempotency key: the same logical call always maps to the same id.
    pub event_id: String,
    pub workspace_id: Uuid,
    pub room_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    /// `llm_suggestion | llm_summary | llm_assistant | embedding`.
    pub feature: &'static str,
    pub model: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    /// `platform | byo`.
    pub boundary: &'static str,
    pub occurred_at: DateTime<Utc>,
    pub segment_id: Option<Uuid>,
}

impl UsageEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn llm(
        event_id: String,
        feature: &'static str,
        workspace: Uuid,
        room: Uuid,
        model: String,
        boundary: DataBoundary,
        tokens_in: u64,
        tokens_out: u64,
        segment_id: Option<Uuid>,
    ) -> Self {
        Self {
            event_id,
            workspace_id: workspace,
            room_id: Some(room),
            user_id: None,
            feature,
            model,
            tokens_in: Some(i64::try_from(tokens_in).unwrap_or(i64::MAX)),
            tokens_out: Some(i64::try_from(tokens_out).unwrap_or(i64::MAX)),
            boundary: boundary_label(boundary),
            occurred_at: Utc::now(),
            segment_id,
        }
    }

    /// Normalized provider usage as `(tokens_in, tokens_out)`.
    pub fn tokens_of(u: &Usage) -> (u64, u64) {
        (u64::from(u.total_input()), u64::from(u.output))
    }
}

pub fn boundary_label(b: DataBoundary) -> &'static str {
    match b {
        DataBoundary::External => "platform",
        DataBoundary::WorkspaceControlled => "byo",
    }
}

#[async_trait]
pub trait UsageSink: Send + Sync {
    async fn emit(&self, event: UsageEvent);
}

pub struct NoUsageSink;

#[async_trait]
impl UsageSink for NoUsageSink {
    async fn emit(&self, _: UsageEvent) {}
}

/// Collects events (tests).
#[derive(Default)]
pub struct MemoryUsageSink {
    pub events: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl UsageSink for MemoryUsageSink {
    async fn emit(&self, event: UsageEvent) {
        if let (Ok(v), Ok(mut g)) = (serde_json::to_value(&event), self.events.lock()) {
            g.push(v);
        }
    }
}

/// Publishes to `usage:events`. A failure is logged and counted, never propagated: billing must
/// not break suggestions or summaries (byo-llm.md section 7, "no failure because of a side task").
pub struct RedisUsageSink {
    conn: ConnectionManager,
}

impl RedisUsageSink {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl UsageSink for RedisUsageSink {
    async fn emit(&self, event: UsageEvent) {
        let mut conn = self.conn.clone();
        if let Err(e) =
            warptalk_bus::xadd_json(&mut conn, USAGE_STREAM, &event, USAGE_STREAM_MAXLEN).await
        {
            metrics::counter!("warptalk_usage_publish_failures_total").increment(1);
            tracing::warn!(error = %e, feature = event.feature, "could not publish usage event");
        }
    }
}

pub type SharedUsageSink = Arc<dyn UsageSink>;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn byo_boundary_and_token_split() {
        assert_eq!(boundary_label(DataBoundary::External), "platform");
        assert_eq!(boundary_label(DataBoundary::WorkspaceControlled), "byo");
        let u = Usage {
            input_uncached: 10,
            cache_read: 5,
            cache_write: 1,
            output: 7,
            reasoning: 3,
        };
        assert_eq!(UsageEvent::tokens_of(&u), (16, 7));
        let e = UsageEvent::llm(
            "x".into(),
            "llm_summary",
            Uuid::nil(),
            Uuid::nil(),
            "m".into(),
            DataBoundary::WorkspaceControlled,
            1,
            2,
            None,
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            (v["boundary"].as_str(), v["tokens_out"].as_i64()),
            (Some("byo"), Some(2))
        );
    }
}
