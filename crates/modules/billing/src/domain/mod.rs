//! Pure billing rules: usage events, rate card pricing, service-state evaluation, entitlements.
//! No IO.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Redis stream the producers write and the worker consumes.
pub const USAGE_STREAM: &str = "usage:events";

pub const CURRENCY_CREDITS: &str = "CRD";
pub const UNIT_AUDIO_SECOND: &str = "audio_second";
pub const UNIT_TOKENS_IN: &str = "1k_tokens_in";
pub const UNIT_TOKENS_OUT: &str = "1k_tokens_out";

// ------------------------------------------------------------------ usage events

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Stt,
    LlmSuggestion,
    LlmSummary,
    LlmAssistant,
    Embedding,
}

impl Feature {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::LlmSuggestion => "llm_suggestion",
            Self::LlmSummary => "llm_summary",
            Self::LlmAssistant => "llm_assistant",
            Self::Embedding => "embedding",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "stt" => Self::Stt,
            "llm_suggestion" => Self::LlmSuggestion,
            "llm_summary" => Self::LlmSummary,
            "llm_assistant" => Self::LlmAssistant,
            "embedding" => Self::Embedding,
            _ => return None,
        })
    }
}

/// Who pays for the provider call: the platform (charged in credits) or the customer's own key
/// (recorded with cost 0, byo-llm.md section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Platform,
    Byo,
}

impl Boundary {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Byo => "byo",
        }
    }
}

/// One entry of `usage:events` (JSON in the stream field `data`). `event_id` is the idempotency key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageEvent {
    pub event_id: String,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    pub feature: Feature,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<i64>,
    pub boundary: Boundary,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<Uuid>,
}

impl UsageEvent {
    pub fn audio_seconds(&self) -> f64 {
        self.audio_ms.unwrap_or(0).max(0) as f64 / 1000.0
    }

    pub fn is_empty(&self) -> bool {
        self.audio_ms.unwrap_or(0) <= 0
            && self.tokens_in.unwrap_or(0) <= 0
            && self.tokens_out.unwrap_or(0) <= 0
    }

    /// `(unit stored on the usage record, quantity in that unit)`.
    pub fn record_quantity(&self) -> (&'static str, f64) {
        if self.feature == Feature::Stt {
            ("audio_second", self.audio_seconds())
        } else {
            (
                "token",
                (self.tokens_in.unwrap_or(0).max(0) + self.tokens_out.unwrap_or(0).max(0)) as f64,
            )
        }
    }
}

// ------------------------------------------------------------------ rate card + pricing

#[derive(Debug, Clone, PartialEq)]
pub struct RateEntry {
    pub id: Uuid,
    pub charge_type: String,
    pub unit: String,
    /// `None`: default for any model of the charge type.
    pub model: Option<String>,
    /// Credits per unit.
    pub unit_price: f64,
}

#[derive(Debug, Clone, Default)]
pub struct RateCard {
    pub entries: Vec<RateEntry>,
}

impl RateCard {
    /// Exact model first, then the model-agnostic default.
    pub fn lookup(&self, charge_type: &str, unit: &str, model: &str) -> Option<&RateEntry> {
        let matches = |e: &&RateEntry| e.charge_type == charge_type && e.unit == unit;
        self.entries
            .iter()
            .filter(matches)
            .find(|e| e.model.as_deref() == Some(model))
            .or_else(|| {
                self.entries
                    .iter()
                    .filter(matches)
                    .find(|e| e.model.is_none())
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Priced {
    /// Credits to debit (0 for BYO, or when unpriced).
    pub credits: i32,
    pub rate_id: Option<Uuid>,
    pub unit_price: Option<f64>,
    /// No rate-card entry matched a billable quantity: recorded with 0 credits and flagged.
    pub unpriced: bool,
}

fn ceil_credits(x: f64) -> i64 {
    // Absorb binary floating error (0.1 * 30 = 3.0000000000000004) before rounding up.
    ((x - 1e-9).ceil().max(0.0)) as i64
}

/// Price one event. Platform usage: ceil(quantity x rate), at least 1 credit when anything was
/// used (BR-68). BYO usage costs 0 credits.
pub fn price_event(card: &RateCard, e: &UsageEvent) -> Priced {
    if e.is_empty() {
        return Priced {
            credits: 0,
            rate_id: None,
            unit_price: None,
            unpriced: false,
        };
    }
    let charge = e.feature.as_str();
    let mut raw = 0.0_f64;
    let mut first: Option<&RateEntry> = None;
    let mut missing = false;
    let mut lines: Vec<(&str, f64)> = Vec::new();
    if e.feature == Feature::Stt {
        lines.push((UNIT_AUDIO_SECOND, e.audio_seconds()));
    } else {
        lines.push((
            UNIT_TOKENS_IN,
            e.tokens_in.unwrap_or(0).max(0) as f64 / 1000.0,
        ));
        lines.push((
            UNIT_TOKENS_OUT,
            e.tokens_out.unwrap_or(0).max(0) as f64 / 1000.0,
        ));
    }
    for (unit, qty) in lines {
        if qty <= 0.0 {
            continue;
        }
        match card.lookup(charge, unit, &e.model) {
            Some(r) => {
                raw += qty * r.unit_price;
                first.get_or_insert(r);
            }
            None => missing = true,
        }
    }
    if e.boundary == Boundary::Byo {
        return Priced {
            credits: 0,
            rate_id: first.map(|r| r.id),
            unit_price: first.map(|r| r.unit_price),
            unpriced: false,
        };
    }
    if first.is_none() && missing {
        return Priced {
            credits: 0,
            rate_id: None,
            unit_price: None,
            unpriced: true,
        };
    }
    let credits = ceil_credits(raw).max(1).min(i64::from(i32::MAX)) as i32;
    Priced {
        credits,
        rate_id: first.map(|r| r.id),
        unit_price: first.map(|r| r.unit_price),
        unpriced: missing,
    }
}

// ------------------------------------------------------------------ service state

pub const REASON_INSUFFICIENT_CREDITS: &str = "insufficient_credits";
pub const REASON_OVERAGE_CAP: &str = "overage_cap";
pub const REASON_TRIAL_ENDED: &str = "trial_ended";
pub const REASON_EXPIRED: &str = "subscription_expired";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Healthy,
    LowBalance,
    InOverage,
    Suspended(&'static str),
}

impl ServiceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::LowBalance => "low_balance",
            Self::InOverage => "in_overage",
            Self::Suspended(_) => "suspended",
        }
    }

    pub fn suspended_reason(&self) -> Option<&'static str> {
        match self {
            Self::Suspended(r) => Some(r),
            _ => None,
        }
    }

    pub fn allows_ai(&self) -> bool {
        !matches!(self, Self::Suspended(_))
    }
}

pub struct WalletFacts {
    pub balance: i32,
    pub status: String,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub overage_cap: i32,
    pub low_threshold: i32,
    pub now: DateTime<Utc>,
}

/// BR-66/67: with credits left usage is healthy; at <= 0 it continues only as overage up to the
/// cap, otherwise the AI service is suspended.
pub fn evaluate_state(f: &WalletFacts) -> ServiceState {
    if f.status == "trialing" && f.trial_ends_at.is_some_and(|t| t <= f.now) {
        return ServiceState::Suspended(REASON_TRIAL_ENDED);
    }
    if f.balance <= 0 {
        if f.overage_cap <= 0 {
            return ServiceState::Suspended(REASON_INSUFFICIENT_CREDITS);
        }
        if i64::from(f.balance) <= -i64::from(f.overage_cap) {
            return ServiceState::Suspended(REASON_OVERAGE_CAP);
        }
        if f.balance < 0 {
            return ServiceState::InOverage;
        }
    }
    if f.balance <= f.low_threshold {
        return ServiceState::LowBalance;
    }
    ServiceState::Healthy
}

// ------------------------------------------------------------------ subscription status

/// Map a Stripe subscription status to ours (`active | trialing | past_due | cancelled`).
pub fn map_stripe_status(s: &str) -> &'static str {
    match s {
        "active" => "active",
        "trialing" => "trialing",
        "past_due" | "unpaid" | "incomplete" => "past_due",
        "canceled" | "incomplete_expired" => "cancelled",
        _ => "past_due",
    }
}

// ------------------------------------------------------------------ entitlements

/// Effective entitlements: plan columns and `features`, then per-workspace overrides
/// (`workspace_entitlement_overrides`: `true`/`false` or an integer).
pub fn resolve_entitlements(
    plan_flags: &[(&str, bool)],
    plan_limits: &[(&str, i64)],
    plan_features: &Value,
    overrides: &HashMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for (k, v) in plan_flags {
        out.insert((*k).to_owned(), Value::Bool(*v));
    }
    for (k, v) in plan_limits {
        out.insert((*k).to_owned(), Value::from(*v));
    }
    if let Some(o) = plan_features.as_object() {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in overrides {
        let parsed = match v.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            n => n
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(n.to_owned())),
        };
        out.insert(k.clone(), parsed);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn card() -> RateCard {
        RateCard {
            entries: vec![
                RateEntry {
                    id: Uuid::now_v7(),
                    charge_type: "stt".into(),
                    unit: UNIT_AUDIO_SECOND.into(),
                    model: None,
                    unit_price: 1.0,
                },
                RateEntry {
                    id: Uuid::now_v7(),
                    charge_type: "stt".into(),
                    unit: UNIT_AUDIO_SECOND.into(),
                    model: Some("cheap".into()),
                    unit_price: 0.5,
                },
                RateEntry {
                    id: Uuid::now_v7(),
                    charge_type: "llm_summary".into(),
                    unit: UNIT_TOKENS_IN.into(),
                    model: None,
                    unit_price: 1.0,
                },
                RateEntry {
                    id: Uuid::now_v7(),
                    charge_type: "llm_summary".into(),
                    unit: UNIT_TOKENS_OUT.into(),
                    model: None,
                    unit_price: 3.0,
                },
            ],
        }
    }

    fn ev(feature: Feature, model: &str) -> UsageEvent {
        UsageEvent {
            event_id: "e".into(),
            workspace_id: Uuid::now_v7(),
            room_id: None,
            user_id: None,
            feature,
            model: model.into(),
            audio_ms: None,
            tokens_in: None,
            tokens_out: None,
            boundary: Boundary::Platform,
            occurred_at: Utc::now(),
            segment_id: None,
        }
    }

    #[test]
    fn stt_priced_per_second_with_model_override_and_minimum() {
        let mut e = ev(Feature::Stt, "gpt-x");
        e.audio_ms = Some(12_400);
        assert_eq!(price_event(&card(), &e).credits, 13);
        e.model = "cheap".into();
        assert_eq!(price_event(&card(), &e).credits, 7);
        e.audio_ms = Some(40);
        assert_eq!(
            price_event(&card(), &e).credits,
            1,
            "minimum 1 credit per event"
        );
        e.audio_ms = Some(0);
        assert_eq!(
            price_event(&card(), &e).credits,
            0,
            "nothing used, nothing charged"
        );
    }

    #[test]
    fn llm_prices_input_and_output_separately() {
        let mut e = ev(Feature::LlmSummary, "m");
        e.tokens_in = Some(2_000);
        e.tokens_out = Some(1_000);
        let p = price_event(&card(), &e);
        assert_eq!(p.credits, 5);
        assert!(p.rate_id.is_some());
    }

    #[test]
    fn byo_is_free_but_still_traceable() {
        let mut e = ev(Feature::LlmSummary, "m");
        e.tokens_in = Some(5_000);
        e.boundary = Boundary::Byo;
        let p = price_event(&card(), &e);
        assert_eq!(p.credits, 0);
        assert!(p.rate_id.is_some());
    }

    #[test]
    fn unknown_charge_type_is_flagged_not_charged() {
        let mut e = ev(Feature::Embedding, "m");
        e.tokens_in = Some(1_000);
        let p = price_event(&card(), &e);
        assert_eq!((p.credits, p.unpriced), (0, true));
    }

    #[test]
    fn float_noise_does_not_round_up() {
        let c = RateCard {
            entries: vec![RateEntry {
                id: Uuid::now_v7(),
                charge_type: "stt".into(),
                unit: UNIT_AUDIO_SECOND.into(),
                model: None,
                unit_price: 0.1,
            }],
        };
        let mut e = ev(Feature::Stt, "m");
        e.audio_ms = Some(30_000);
        assert_eq!(price_event(&c, &e).credits, 3);
    }

    fn facts(balance: i32, cap: i32) -> WalletFacts {
        WalletFacts {
            balance,
            status: "active".into(),
            trial_ends_at: None,
            overage_cap: cap,
            low_threshold: 100,
            now: Utc::now(),
        }
    }

    #[test]
    fn state_machine() {
        assert_eq!(evaluate_state(&facts(1000, 0)), ServiceState::Healthy);
        assert_eq!(evaluate_state(&facts(50, 0)), ServiceState::LowBalance);
        assert_eq!(
            evaluate_state(&facts(0, 0)),
            ServiceState::Suspended(REASON_INSUFFICIENT_CREDITS)
        );
        assert_eq!(
            evaluate_state(&facts(-5, 0)),
            ServiceState::Suspended(REASON_INSUFFICIENT_CREDITS)
        );
        assert_eq!(evaluate_state(&facts(0, 500)), ServiceState::LowBalance);
        assert_eq!(evaluate_state(&facts(-10, 500)), ServiceState::InOverage);
        assert_eq!(
            evaluate_state(&facts(-500, 500)),
            ServiceState::Suspended(REASON_OVERAGE_CAP)
        );
        let mut t = facts(1000, 0);
        t.status = "trialing".into();
        t.trial_ends_at = Some(t.now - chrono::Duration::days(1));
        assert_eq!(
            evaluate_state(&t),
            ServiceState::Suspended(REASON_TRIAL_ENDED)
        );
        assert!(ServiceState::InOverage.allows_ai());
    }

    #[test]
    fn stripe_status_mapping() {
        assert_eq!(map_stripe_status("canceled"), "cancelled");
        assert_eq!(map_stripe_status("unpaid"), "past_due");
        assert_eq!(map_stripe_status("active"), "active");
    }

    #[test]
    fn event_wire_format_round_trips() {
        let mut e = ev(Feature::LlmSuggestion, "m");
        e.tokens_in = Some(10);
        let j = serde_json::to_string(&e).unwrap();
        assert!(
            j.contains("\"feature\":\"llm_suggestion\"") && j.contains("\"boundary\":\"platform\"")
        );
        assert_eq!(serde_json::from_str::<UsageEvent>(&j).unwrap(), e);
    }

    #[test]
    fn entitlements_overrides_win() {
        let mut o = HashMap::new();
        o.insert("max_concurrent_copilot_sessions".to_owned(), "9".to_owned());
        o.insert("glossary_enabled".to_owned(), "true".to_owned());
        let e = resolve_entitlements(
            &[("glossary_enabled", false)],
            &[],
            &serde_json::json!({"max_concurrent_copilot_sessions": 1}),
            &o,
        );
        assert_eq!(e["max_concurrent_copilot_sessions"], 9);
        assert_eq!(e["glossary_enabled"], true);
    }
}
