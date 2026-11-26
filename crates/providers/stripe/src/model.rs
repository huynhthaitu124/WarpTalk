use serde::Deserialize;

/// `payment` for one-off credit top-ups, `subscription` for plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutMode {
    Payment,
    Subscription,
}

impl CheckoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::Subscription => "subscription",
        }
    }
}

/// One checkout line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutLine {
    /// A Stripe Price (recurring or one-time), quantity 1.
    Price(String),
    /// Ad-hoc amount (top-ups): `unit_amount` in the currency's minor unit (cents).
    Amount {
        name: String,
        currency: String,
        unit_amount: i64,
    },
}

#[derive(Debug, Clone)]
pub struct CheckoutRequest {
    pub mode: CheckoutMode,
    pub line: CheckoutLine,
    pub success_url: String,
    pub cancel_url: String,
    pub customer: Option<String>,
    pub client_reference_id: Option<String>,
    /// Copied onto the session and, for subscriptions/payments, onto the subscription/payment
    /// intent so later events carry them.
    pub metadata: Vec<(String, String)>,
}

impl CheckoutRequest {
    /// Form fields as Stripe expects them (bracket notation for nesting).
    pub(crate) fn form(&self) -> Vec<(String, String)> {
        let mut f: Vec<(String, String)> = vec![
            ("mode".into(), self.mode.as_str().into()),
            ("success_url".into(), self.success_url.clone()),
            ("cancel_url".into(), self.cancel_url.clone()),
            ("line_items[0][quantity]".into(), "1".into()),
        ];
        match &self.line {
            CheckoutLine::Price(id) => f.push(("line_items[0][price]".into(), id.clone())),
            CheckoutLine::Amount {
                name,
                currency,
                unit_amount,
            } => {
                f.push((
                    "line_items[0][price_data][currency]".into(),
                    currency.to_lowercase(),
                ));
                f.push((
                    "line_items[0][price_data][unit_amount]".into(),
                    unit_amount.to_string(),
                ));
                f.push((
                    "line_items[0][price_data][product_data][name]".into(),
                    name.clone(),
                ));
            }
        }
        if let Some(c) = &self.customer {
            f.push(("customer".into(), c.clone()));
        }
        if let Some(r) = &self.client_reference_id {
            f.push(("client_reference_id".into(), r.clone()));
        }
        let nested = match self.mode {
            CheckoutMode::Subscription => "subscription_data[metadata]",
            CheckoutMode::Payment => "payment_intent_data[metadata]",
        };
        for (k, v) in &self.metadata {
            f.push((format!("metadata[{k}]"), v.clone()));
            f.push((format!("{nested}[{k}]"), v.clone()));
        }
        f
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CheckoutSession {
    pub id: String,
    /// Hosted checkout page; the client redirects here.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Customer {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BillingPortalSession {
    pub id: String,
    pub url: String,
}

/// Subset of the Subscription object we read.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    pub id: String,
    /// `active | trialing | past_due | canceled | unpaid | incomplete | incomplete_expired | paused`.
    pub status: String,
    pub customer: String,
    #[serde(default)]
    pub cancel_at_period_end: bool,
    /// UNVERIFIED: newer API versions moved the period to `items.data[].current_period_*`;
    /// [`Subscription::period`] reads both.
    #[serde(default)]
    pub current_period_start: Option<i64>,
    #[serde(default)]
    pub current_period_end: Option<i64>,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub items: SubscriptionItems,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct SubscriptionItems {
    #[serde(default)]
    pub data: Vec<SubscriptionItem>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SubscriptionItem {
    #[serde(default)]
    pub price: Option<ItemPrice>,
    #[serde(default)]
    pub current_period_start: Option<i64>,
    #[serde(default)]
    pub current_period_end: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ItemPrice {
    pub id: String,
}

impl Subscription {
    /// `(start, end)` unix seconds of the current period.
    pub fn period(&self) -> Option<(i64, i64)> {
        let item = self.items.data.first();
        let start = self
            .current_period_start
            .or_else(|| item.and_then(|i| i.current_period_start))?;
        let end = self
            .current_period_end
            .or_else(|| item.and_then(|i| i.current_period_end))?;
        Some((start, end))
    }

    /// Price id of the first item.
    pub fn price_id(&self) -> Option<&str> {
        self.items
            .data
            .first()
            .and_then(|i| i.price.as_ref())
            .map(|p| p.id.as_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn subscription_period_from_item_or_top_level() {
        let top: Subscription = serde_json::from_value(serde_json::json!({
            "id": "sub_1", "status": "active", "customer": "cus_1",
            "current_period_start": 10, "current_period_end": 20,
            "items": {"data": [{"price": {"id": "price_x"}}]}
        }))
        .unwrap();
        assert_eq!(top.period(), Some((10, 20)));
        assert_eq!(top.price_id(), Some("price_x"));
        let item: Subscription = serde_json::from_value(serde_json::json!({
            "id": "sub_1", "status": "active", "customer": "cus_1",
            "items": {"data": [{"current_period_start": 1, "current_period_end": 2}]}
        }))
        .unwrap();
        assert_eq!(item.period(), Some((1, 2)));
    }

    #[test]
    fn checkout_form_for_price_and_amount() {
        let r = CheckoutRequest {
            mode: CheckoutMode::Subscription,
            line: CheckoutLine::Price("price_1".into()),
            success_url: "https://a/ok".into(),
            cancel_url: "https://a/no".into(),
            customer: Some("cus_1".into()),
            client_reference_id: Some("ws".into()),
            metadata: vec![("workspace_id".into(), "w1".into())],
        };
        let f = r.form();
        assert!(f.contains(&("mode".into(), "subscription".into())));
        assert!(f.contains(&("line_items[0][price]".into(), "price_1".into())));
        assert!(f.contains(&(
            "subscription_data[metadata][workspace_id]".into(),
            "w1".into()
        )));
        let r = CheckoutRequest {
            mode: CheckoutMode::Payment,
            line: CheckoutLine::Amount {
                name: "Credits".into(),
                currency: "USD".into(),
                unit_amount: 500,
            },
            customer: None,
            client_reference_id: None,
            ..r
        };
        let f = r.form();
        assert!(f.contains(&("line_items[0][price_data][currency]".into(), "usd".into())));
        assert!(f.contains(&(
            "line_items[0][price_data][unit_amount]".into(),
            "500".into()
        )));
        assert!(f.contains(&(
            "payment_intent_data[metadata][workspace_id]".into(),
            "w1".into()
        )));
    }
}
