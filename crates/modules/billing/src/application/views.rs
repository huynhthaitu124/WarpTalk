//! Response models (camelCase JSON).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::ports::{PlanRow, TransactionRow, UsageGroup, UsageTotals};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
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
    pub overage_cap_credits: i32,
    pub rollover_cap_credits: i32,
    pub features: Value,
    /// A Stripe price id is configured, so the plan can be purchased.
    pub purchasable: bool,
}

impl PlanView {
    pub fn from_row(p: &PlanRow, purchasable: bool) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            slug: p.slug.clone(),
            tier: p.tier.clone(),
            price: p.price,
            currency: p.currency.clone(),
            billing_cycle: p.billing_cycle.clone(),
            credits_per_cycle: p.credits_per_cycle,
            max_participants: p.max_participants,
            max_languages: p.max_languages,
            max_active_rooms: p.max_active_rooms,
            overage_cap_credits: p.overage_cap_credits,
            rollover_cap_credits: p.rollover_cap_credits,
            features: p.features.clone(),
            purchasable,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub id: Uuid,
    pub plan: PlanView,
    /// `trialing | active | past_due | cancelled`.
    pub status: String,
    pub current_period_start: DateTime<Utc>,
    pub current_period_end: DateTime<Utc>,
    pub auto_renew: bool,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriodView {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub credits_consumed: i64,
    pub audio_seconds: f64,
    pub tokens: i64,
    pub events: i64,
}

impl UsagePeriodView {
    pub fn new(from: DateTime<Utc>, to: DateTime<Utc>, t: UsageTotals) -> Self {
        Self {
            from,
            to,
            credits_consumed: t.credits,
            audio_seconds: t.audio_seconds,
            tokens: t.tokens,
            events: t.events,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingOverview {
    pub workspace_id: Uuid,
    pub subscription: SubscriptionView,
    pub balance: i32,
    pub credits_used_this_cycle: i32,
    /// `healthy | low_balance | in_overage | suspended`.
    pub service_state: String,
    pub suspended_reason: Option<String>,
    pub usage: UsagePeriodView,
    pub entitlements: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutUrl {
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroupView {
    pub key: String,
    pub credits: i64,
    pub quantity: f64,
    pub events: i64,
}

impl From<UsageGroup> for UsageGroupView {
    fn from(g: UsageGroup) -> Self {
        Self {
            key: g.key,
            credits: g.credits,
            quantity: g.quantity,
            events: g.events,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub group_by: String,
    pub groups: Vec<UsageGroupView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionView {
    pub id: Uuid,
    pub amount: i32,
    #[serde(rename = "type")]
    pub tx_type: String,
    pub charge_type: Option<String>,
    pub description: Option<String>,
    pub balance_after: i32,
    pub reference_id: Option<Uuid>,
    pub reference_type: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<TransactionRow> for TransactionView {
    fn from(t: TransactionRow) -> Self {
        Self {
            id: t.id,
            amount: t.amount,
            tx_type: t.tx_type,
            charge_type: t.charge_type,
            description: t.description,
            balance_after: t.balance_after,
            reference_id: t.reference_id,
            reference_type: t.reference_type,
            created_at: t.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookAck {
    pub received: bool,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustResult {
    pub balance: i32,
    pub duplicate: bool,
    pub service_state: String,
}
