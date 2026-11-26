//! Ports and read models of the billing module.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};
use warptalk_stripe::{CheckoutRequest, CheckoutSession, StripeError, Subscription};

use crate::domain::{Priced, RateCard, ServiceState, UsageEvent};

// ------------------------------------------------------------------ outbound ports

/// Workspace membership (api wires the workspace module's read facade).
#[async_trait]
pub trait WorkspaceAccess: Send + Sync {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>>;
}

/// Payment provider operations used by the checkout/portal flows.
#[async_trait]
pub trait PaymentGateway: Send + Sync {
    async fn create_customer(
        &self,
        ws: WorkspaceId,
        idempotency_key: &str,
    ) -> Result<String, StripeError>;
    async fn create_checkout(
        &self,
        req: &CheckoutRequest,
        idempotency_key: &str,
    ) -> Result<CheckoutSession, StripeError>;
    async fn create_portal(&self, customer: &str, return_url: &str) -> Result<String, StripeError>;
    async fn subscription(&self, id: &str) -> Result<Subscription, StripeError>;
}

/// Publishes the workspace's AI service state where other services read it (Redis keys
/// `workspace:{id}:ai_service_state` / `:ai_service_suspended`). Failures are logged, never fatal.
#[async_trait]
pub trait ServiceStateSink: Send + Sync {
    async fn publish(&self, ws: WorkspaceId, state: &ServiceState);
}

pub struct NoStateSink;

#[async_trait]
impl ServiceStateSink for NoStateSink {
    async fn publish(&self, _: WorkspaceId, _: &ServiceState) {}
}

// ------------------------------------------------------------------ read models

#[derive(Debug, Clone)]
pub struct PlanRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub tier: String,
    pub price: f64,
    pub currency: String,
    pub billing_cycle: String,
    pub credits_per_cycle: i32,
    pub max_participants: i32,
    pub max_languages: i32,
    pub max_active_rooms: i32,
    pub voice_clone_enabled: bool,
    pub ai_assistant_enabled: bool,
    pub glossary_enabled: bool,
    pub dedicated_gpu: bool,
    pub overage_cap_credits: i32,
    pub low_balance_threshold_credits: i32,
    pub rollover_cap_credits: i32,
    pub features: Value,
    pub sort_order: i32,
    pub is_active: bool,
    pub stripe_price_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubscriptionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub plan_id: Uuid,
    pub status: String,
    pub credits_remaining: i32,
    pub credits_used_this_cycle: i32,
    pub current_period_start: DateTime<Utc>,
    pub current_period_end: DateTime<Utc>,
    pub auto_renew: bool,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub credits_per_cycle_override: Option<i32>,
    pub overage_cap_override: Option<i32>,
    pub low_balance_override: Option<i32>,
    pub overage_credits_this_cycle: i32,
    pub service_state: String,
    pub suspended_reason: Option<String>,
    pub stripe_subscription_id: Option<String>,
}

impl SubscriptionRow {
    pub fn overage_cap(&self, plan: &PlanRow) -> i32 {
        self.overage_cap_override
            .unwrap_or(plan.overage_cap_credits)
    }
    pub fn low_threshold(&self, plan: &PlanRow) -> i32 {
        self.low_balance_override
            .unwrap_or(plan.low_balance_threshold_credits)
    }
    pub fn credits_per_cycle(&self, plan: &PlanRow) -> i32 {
        self.credits_per_cycle_override
            .unwrap_or(plan.credits_per_cycle)
    }
}

#[derive(Debug, Clone)]
pub struct TransactionRow {
    pub id: Uuid,
    pub amount: i32,
    pub tx_type: String,
    pub charge_type: Option<String>,
    pub description: Option<String>,
    pub balance_after: i32,
    pub reference_id: Option<Uuid>,
    pub reference_type: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Day,
    Feature,
    Member,
}

#[derive(Debug, Clone)]
pub struct UsageGroup {
    pub key: String,
    pub credits: i64,
    pub quantity: f64,
    pub events: i64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub credits: i64,
    pub audio_seconds: f64,
    pub tokens: i64,
    pub events: i64,
}

#[derive(Debug, Clone)]
pub struct RateCardInput {
    pub charge_type: String,
    pub unit: String,
    pub model: Option<String>,
    pub unit_price: f64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageOutcome {
    pub duplicate: bool,
    pub credits: i32,
    pub state: ServiceState,
}

#[derive(Debug, Clone)]
pub struct AdjustOutcome {
    pub duplicate: bool,
    pub balance: i32,
    pub state: ServiceState,
}

/// What a verified Stripe event does to our state (decided by the application layer, applied
/// atomically with the event-id dedup row by the store).
#[derive(Debug, Clone)]
pub enum WebhookEffect {
    Ignore,
    CheckoutSubscription {
        ws: WorkspaceId,
        plan_id: Uuid,
        stripe_subscription_id: String,
        customer: Option<String>,
        status: String,
        period: Option<(DateTime<Utc>, DateTime<Utc>)>,
        session_id: String,
    },
    TopUp {
        ws: WorkspaceId,
        credits: i32,
        session_id: String,
        amount_cents: i64,
        currency: String,
    },
    InvoicePaid {
        ws: WorkspaceId,
        invoice_id: String,
        stripe_subscription_id: Option<String>,
        plan_id: Option<Uuid>,
        /// `subscription_create` / `subscription_cycle` grant the plan's credits.
        grant_credits: bool,
        /// Renewal (`subscription_cycle`): credits above the rollover cap expire first. The
        /// first invoice keeps unused trial credits.
        apply_rollover: bool,
        period: Option<(DateTime<Utc>, DateTime<Utc>)>,
        amount_cents: i64,
        currency: String,
        hosted_invoice_url: Option<String>,
        invoice_pdf_url: Option<String>,
    },
    InvoiceFailed {
        ws: WorkspaceId,
        invoice_id: String,
        amount_cents: i64,
        currency: String,
        reason: Option<String>,
    },
    SubscriptionUpdated {
        ws: WorkspaceId,
        stripe_subscription_id: String,
        status: String,
        cancel_at_period_end: bool,
        period: Option<(DateTime<Utc>, DateTime<Utc>)>,
        plan_id: Option<Uuid>,
    },
    SubscriptionDeleted {
        ws: WorkspaceId,
        stripe_subscription_id: String,
    },
}

impl WebhookEffect {
    pub fn workspace(&self) -> Option<WorkspaceId> {
        match self {
            Self::Ignore => None,
            Self::CheckoutSubscription { ws, .. }
            | Self::TopUp { ws, .. }
            | Self::InvoicePaid { ws, .. }
            | Self::InvoiceFailed { ws, .. }
            | Self::SubscriptionUpdated { ws, .. }
            | Self::SubscriptionDeleted { ws, .. } => Some(*ws),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebhookOutcome {
    /// `true` when this event id was already processed (replay): nothing changed.
    pub duplicate: bool,
    /// State of the affected workspace after the effect, if any.
    pub state: Option<(WorkspaceId, ServiceState)>,
}

// ------------------------------------------------------------------ store port

#[async_trait]
pub trait BillingStore: Send + Sync {
    async fn list_plans(&self, active_only: bool) -> AppResult<Vec<PlanRow>>;
    async fn plan_by_id(&self, id: Uuid) -> AppResult<Option<PlanRow>>;
    async fn plan_by_slug(&self, slug: &str) -> AppResult<Option<PlanRow>>;

    /// The workspace's subscription (creating the free-trial wallet and granting its credits on
    /// first contact, once).
    async fn ensure_subscription(
        &self,
        ws: WorkspaceId,
        user: Option<UserId>,
        now: DateTime<Utc>,
    ) -> AppResult<(SubscriptionRow, PlanRow)>;

    async fn customer_id(&self, ws: WorkspaceId) -> AppResult<Option<String>>;
    /// Stores the customer id unless one exists; returns the id that is stored.
    async fn save_customer(&self, ws: WorkspaceId, customer: &str) -> AppResult<String>;
    async fn workspace_for_customer(&self, customer: &str) -> AppResult<Option<WorkspaceId>>;
    async fn workspace_for_stripe_subscription(&self, id: &str) -> AppResult<Option<WorkspaceId>>;
    async fn overrides(&self, ws: WorkspaceId) -> AppResult<HashMap<String, String>>;

    async fn usage_totals(
        &self,
        ws: WorkspaceId,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AppResult<UsageTotals>;
    async fn usage_grouped(
        &self,
        ws: WorkspaceId,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        by: GroupBy,
    ) -> AppResult<Vec<UsageGroup>>;
    async fn transactions(
        &self,
        ws: WorkspaceId,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<TransactionRow>, i64)>;

    async fn rate_card(&self) -> AppResult<RateCard>;
    async fn replace_rate_card(
        &self,
        actor: UserId,
        entries: &[RateCardInput],
        now: DateTime<Utc>,
    ) -> AppResult<()>;

    /// Record the usage and debit the wallet in one transaction, once per `event_id`.
    async fn record_usage(
        &self,
        e: &UsageEvent,
        priced: &Priced,
        now: DateTime<Utc>,
    ) -> AppResult<UsageOutcome>;

    async fn apply_webhook(
        &self,
        event_id: &str,
        event_type: &str,
        effect: WebhookEffect,
        now: DateTime<Utc>,
    ) -> AppResult<WebhookOutcome>;

    /// Admin adjustment (audited in the same transaction).
    async fn adjust_credits(
        &self,
        ws: WorkspaceId,
        amount: i32,
        reason: &str,
        actor: UserId,
        idempotency_key: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<AdjustOutcome>;

    /// Re-evaluate and persist the service state from the current wallet (time-dependent rules
    /// such as trial expiry).
    async fn refresh_state(&self, ws: WorkspaceId, now: DateTime<Utc>) -> AppResult<ServiceState>;
}
