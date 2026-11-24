//! Use cases: plan catalog, billing overview, checkout / portal, usage and ledger queries,
//! Stripe webhook handling and admin operations.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;
use uuid::Uuid;
use warptalk_kernel::{
    AppError, AppResult, Clock, Page, PageRequest, UserId, WorkspaceId, WorkspaceRole,
};
use warptalk_stripe::{CheckoutLine, CheckoutMode, CheckoutRequest, StripeError};

use super::ports::{
    AdjustOutcome, BillingStore, GroupBy, PaymentGateway, PlanRow, RateCardInput,
    ServiceStateSink, SubscriptionRow, WebhookEffect, WorkspaceAccess,
};
use super::views::{
    AdjustResult, BillingOverview, CheckoutUrl, PlanView, SubscriptionView, TransactionView,
    UsagePeriodView, UsageReport, WebhookAck,
};
use crate::domain::{
    Feature, ServiceState, UNIT_AUDIO_SECOND, UNIT_TOKENS_IN, UNIT_TOKENS_OUT, WalletFacts,
    evaluate_state, resolve_entitlements,
};

/// Settings of the billing flows (env-driven in `apps/api`; never contains secrets other than the
/// webhook secret, which is never logged).
#[derive(Clone)]
pub struct BillingConfig {
    pub webhook_secret: Option<String>,
    pub webhook_tolerance_secs: i64,
    /// Plan slug -> Stripe price id (`STRIPE_PRICE_<SLUG>`); falls back to `plans.stripe_price_id`.
    pub price_ids: HashMap<String, String>,
    pub success_url: String,
    pub cancel_url: String,
    pub portal_return_url: String,
    /// Credit top-up price in `currency` per credit.
    pub topup_price_per_credit: f64,
    pub topup_min_credits: i64,
    pub topup_max_credits: i64,
    pub currency: String,
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self {
            webhook_secret: None,
            webhook_tolerance_secs: warptalk_stripe::webhook::DEFAULT_TOLERANCE_SECS,
            price_ids: HashMap::new(),
            success_url: "http://localhost:3000/billing?checkout=success".into(),
            cancel_url: "http://localhost:3000/billing?checkout=cancelled".into(),
            portal_return_url: "http://localhost:3000/billing".into(),
            topup_price_per_credit: 0.0002,
            topup_min_credits: 1_000,
            topup_max_credits: 5_000_000,
            currency: "USD".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CheckoutTarget {
    Plan(Uuid),
    TopUp(i64),
}

#[derive(Debug, Clone)]
pub struct RateCardEdit {
    pub charge_type: String,
    pub unit: String,
    pub model: Option<String>,
    pub unit_price: f64,
    pub notes: Option<String>,
}

pub struct BillingService {
    store: Arc<dyn BillingStore>,
    access: Arc<dyn WorkspaceAccess>,
    gateway: Option<Arc<dyn PaymentGateway>>,
    sink: Arc<dyn ServiceStateSink>,
    clock: Arc<dyn Clock>,
    cfg: BillingConfig,
}

fn stripe_err(e: StripeError) -> AppError {
    tracing::warn!(error = %e, "stripe call failed");
    AppError::coded(
        502,
        "PAYMENT_PROVIDER_ERROR",
        "The payment provider did not complete the request",
    )
}

fn not_configured() -> AppError {
    AppError::coded(
        503,
        "STRIPE_NOT_CONFIGURED",
        "Payments are not configured on this server",
    )
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn ts(secs: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(secs, 0).single()
}

fn period_of(start: Option<i64>, end: Option<i64>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    Some((ts(start?)?, ts(end?)?))
}

impl BillingService {
    pub fn new(
        store: Arc<dyn BillingStore>,
        access: Arc<dyn WorkspaceAccess>,
        gateway: Option<Arc<dyn PaymentGateway>>,
        sink: Arc<dyn ServiceStateSink>,
        clock: Arc<dyn Clock>,
        cfg: BillingConfig,
    ) -> Self {
        Self {
            store,
            access,
            gateway,
            sink,
            clock,
            cfg,
        }
    }

    fn price_id(&self, plan: &PlanRow) -> Option<String> {
        self.cfg
            .price_ids
            .get(&plan.slug)
            .cloned()
            .or_else(|| plan.stripe_price_id.clone())
            .filter(|p| !p.trim().is_empty())
    }

    // ---------------------------------------------------------------- access

    async fn role(&self, ws: WorkspaceId, user: UserId) -> AppResult<WorkspaceRole> {
        self.access
            .member_role(ws, user)
            .await?
            .ok_or_else(|| AppError::forbidden("Not a member of this workspace"))
    }

    async fn require_manager(&self, ws: WorkspaceId, user: UserId) -> AppResult<()> {
        match self.role(ws, user).await? {
            WorkspaceRole::Owner | WorkspaceRole::Admin => Ok(()),
            WorkspaceRole::Member => Err(AppError::forbidden(
                "Only workspace owners and admins can view this",
            )),
        }
    }

    async fn require_owner(&self, ws: WorkspaceId, user: UserId) -> AppResult<()> {
        match self.role(ws, user).await? {
            WorkspaceRole::Owner => Ok(()),
            _ => Err(AppError::forbidden("Only the workspace owner can do this")),
        }
    }

    // ---------------------------------------------------------------- queries

    pub async fn plans(&self) -> AppResult<Vec<PlanView>> {
        Ok(self
            .store
            .list_plans(true)
            .await?
            .iter()
            .map(|p| PlanView::from_row(p, p.price <= 0.0 || self.price_id(p).is_some()))
            .collect())
    }

    pub async fn overview(&self, ws: WorkspaceId, user: UserId) -> AppResult<BillingOverview> {
        self.role(ws, user).await?;
        let now = self.clock.now();
        let (sub, plan) = self.store.ensure_subscription(ws, Some(user), now).await?;
        let state = self.state_of(&sub, &plan, now);
        let overrides = self.store.overrides(ws).await?;
        let totals = self
            .store
            .usage_totals(ws, sub.current_period_start, now + Duration::seconds(1))
            .await?;
        let entitlements = resolve_entitlements(
            &[
                ("voice_clone_enabled", plan.voice_clone_enabled),
                ("ai_assistant_enabled", plan.ai_assistant_enabled),
                ("glossary_enabled", plan.glossary_enabled),
                ("dedicated_gpu", plan.dedicated_gpu),
            ],
            &[
                ("max_participants", i64::from(plan.max_participants)),
                ("max_languages", i64::from(plan.max_languages)),
                ("max_active_rooms", i64::from(plan.max_active_rooms)),
                ("credits_per_cycle", i64::from(sub.credits_per_cycle(&plan))),
            ],
            &plan.features,
            &overrides,
        );
        Ok(BillingOverview {
            workspace_id: ws.as_uuid(),
            balance: sub.credits_remaining,
            credits_used_this_cycle: sub.credits_used_this_cycle,
            service_state: state.as_str().to_owned(),
            suspended_reason: state.suspended_reason().map(str::to_owned),
            usage: UsagePeriodView::new(sub.current_period_start, now + Duration::seconds(1), totals),
            entitlements,
            subscription: SubscriptionView {
                id: sub.id,
                plan: PlanView::from_row(&plan, true),
                status: sub.status.clone(),
                current_period_start: sub.current_period_start,
                current_period_end: sub.current_period_end,
                auto_renew: sub.auto_renew,
                trial_ends_at: sub.trial_ends_at,
                cancelled_at: sub.cancelled_at,
            },
        })
    }

    fn state_of(&self, sub: &SubscriptionRow, plan: &PlanRow, now: DateTime<Utc>) -> ServiceState {
        evaluate_state(&WalletFacts {
            balance: sub.credits_remaining,
            status: sub.status.clone(),
            trial_ends_at: sub.trial_ends_at,
            overage_cap: sub.overage_cap(plan),
            low_threshold: sub.low_threshold(plan),
            now,
        })
    }

    /// Current AI service state of a workspace (the credit gate used by copilot).
    pub async fn ai_state(&self, ws: WorkspaceId) -> AppResult<ServiceState> {
        let now = self.clock.now();
        let (sub, plan) = self.store.ensure_subscription(ws, None, now).await?;
        Ok(self.state_of(&sub, &plan, now))
    }

    pub async fn usage(
        &self,
        ws: WorkspaceId,
        user: UserId,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        group_by: &str,
    ) -> AppResult<UsageReport> {
        self.require_manager(ws, user).await?;
        let by = match group_by {
            "" | "day" => GroupBy::Day,
            "feature" => GroupBy::Feature,
            "member" => GroupBy::Member,
            _ => {
                return Err(AppError::validation(
                    "INVALID_GROUP_BY",
                    "groupBy must be day, feature or member",
                ));
            }
        };
        let now = self.clock.now();
        let to = to.unwrap_or(now + Duration::seconds(1));
        let from = from.unwrap_or(to - Duration::days(30));
        if from >= to {
            return Err(AppError::validation("INVALID_RANGE", "from must be before to"));
        }
        let groups = self.store.usage_grouped(ws, from, to, by).await?;
        Ok(UsageReport {
            from,
            to,
            group_by: match by {
                GroupBy::Day => "day",
                GroupBy::Feature => "feature",
                GroupBy::Member => "member",
            }
            .to_owned(),
            groups: groups.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn transactions(
        &self,
        ws: WorkspaceId,
        user: UserId,
        page: &PageRequest,
    ) -> AppResult<Page<TransactionView>> {
        self.require_manager(ws, user).await?;
        let (rows, total) = self
            .store
            .transactions(ws, page.limit(), page.offset())
            .await?;
        Ok(Page::new(
            rows.into_iter().map(Into::into).collect(),
            page,
            total,
        ))
    }

    // ---------------------------------------------------------------- checkout / portal

    async fn customer_for(&self, gw: &Arc<dyn PaymentGateway>, ws: WorkspaceId) -> AppResult<String> {
        if let Some(c) = self.store.customer_id(ws).await? {
            return Ok(c);
        }
        let created = gw
            .create_customer(ws, &format!("customer:{ws}"))
            .await
            .map_err(stripe_err)?;
        self.store.save_customer(ws, &created).await
    }

    /// OWNER only (BR-73, "only Owner pays"). Returns the hosted checkout URL.
    pub async fn checkout(
        &self,
        ws: WorkspaceId,
        user: UserId,
        target: CheckoutTarget,
    ) -> AppResult<CheckoutUrl> {
        self.require_owner(ws, user).await?;
        let gw = self.gateway.as_ref().ok_or_else(not_configured)?;
        let now = self.clock.now();
        let (sub, current_plan) = self.store.ensure_subscription(ws, Some(user), now).await?;
        let bucket = now.timestamp() / 300;
        let mut metadata = vec![
            ("workspace_id".to_owned(), ws.to_string()),
            ("initiated_by".to_owned(), user.to_string()),
        ];
        let (mode, line, key) = match target {
            CheckoutTarget::Plan(plan_id) => {
                let plan = self
                    .store
                    .plan_by_id(plan_id)
                    .await?
                    .filter(|p| p.is_active)
                    .ok_or_else(|| AppError::not_found("Plan not found"))?;
                if plan.price <= 0.0 {
                    return Err(AppError::validation(
                        "PLAN_NOT_PURCHASABLE",
                        "This plan cannot be purchased",
                    ));
                }
                // BR-65: one live subscription; changes of a running paid plan go through the portal.
                if sub.status == "active" && sub.stripe_subscription_id.is_some() {
                    let code = if current_plan.id == plan.id {
                        "ALREADY_SUBSCRIBED"
                    } else {
                        "USE_PORTAL_TO_CHANGE_PLAN"
                    };
                    return Err(AppError::conflict(
                        code,
                        "The workspace already has an active subscription; manage it in the billing portal",
                    ));
                }
                let price = self.price_id(&plan).ok_or_else(|| {
                    AppError::coded(
                        503,
                        "PRICE_NOT_CONFIGURED",
                        "No Stripe price is configured for this plan",
                    )
                })?;
                metadata.push(("kind".into(), "subscription".into()));
                metadata.push(("plan_id".into(), plan.id.to_string()));
                metadata.push(("plan_slug".into(), plan.slug.clone()));
                (
                    CheckoutMode::Subscription,
                    CheckoutLine::Price(price),
                    format!("checkout:{ws}:{}:{bucket}", plan.id),
                )
            }
            CheckoutTarget::TopUp(credits) => {
                if !(self.cfg.topup_min_credits..=self.cfg.topup_max_credits).contains(&credits) {
                    return Err(AppError::validation(
                        "INVALID_TOP_UP",
                        format!(
                            "topUpCredits must be between {} and {}",
                            self.cfg.topup_min_credits, self.cfg.topup_max_credits
                        ),
                    ));
                }
                if current_plan
                    .features
                    .get("top_up_enabled")
                    .and_then(Value::as_bool)
                    == Some(false)
                {
                    return Err(AppError::forbidden(
                        "Credit top-ups need a paid plan",
                    ));
                }
                // Stripe's minimum charge is 0.50 in most currencies.
                let cents = ((credits as f64 * self.cfg.topup_price_per_credit * 100.0).ceil() as i64)
                    .max(50);
                metadata.push(("kind".into(), "top_up".into()));
                metadata.push(("top_up_credits".into(), credits.to_string()));
                (
                    CheckoutMode::Payment,
                    CheckoutLine::Amount {
                        name: format!("{credits} WarpTalk credits"),
                        currency: self.cfg.currency.clone(),
                        unit_amount: cents,
                    },
                    format!("checkout:{ws}:topup:{credits}:{bucket}"),
                )
            }
        };
        let customer = self.customer_for(gw, ws).await?;
        let session = gw
            .create_checkout(
                &CheckoutRequest {
                    mode,
                    line,
                    success_url: self.cfg.success_url.clone(),
                    cancel_url: self.cfg.cancel_url.clone(),
                    customer: Some(customer),
                    client_reference_id: Some(ws.to_string()),
                    metadata,
                },
                &key,
            )
            .await
            .map_err(stripe_err)?;
        let url = session.url.ok_or_else(|| {
            AppError::coded(502, "PAYMENT_PROVIDER_ERROR", "Checkout session has no URL")
        })?;
        Ok(CheckoutUrl { url })
    }

    pub async fn portal(&self, ws: WorkspaceId, user: UserId) -> AppResult<CheckoutUrl> {
        self.require_owner(ws, user).await?;
        let gw = self.gateway.as_ref().ok_or_else(not_configured)?;
        let customer = self.store.customer_id(ws).await?.ok_or_else(|| {
            AppError::conflict(
                "NO_BILLING_ACCOUNT",
                "This workspace has no billing account yet; purchase a plan first",
            )
        })?;
        let url = gw
            .create_portal(&customer, &self.cfg.portal_return_url)
            .await
            .map_err(stripe_err)?;
        Ok(CheckoutUrl { url })
    }

    // ---------------------------------------------------------------- webhook

    /// Verify the signature over the RAW body, then apply the event idempotently (NFR-IF-07:
    /// state changes only from verified events). Unknown event types are acknowledged.
    pub async fn handle_webhook(
        &self,
        raw: &[u8],
        signature_header: Option<&str>,
    ) -> AppResult<WebhookAck> {
        let secret = self
            .cfg
            .webhook_secret
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(not_configured)?;
        let now = self.clock.now();
        let header = signature_header
            .ok_or_else(|| AppError::validation("INVALID_SIGNATURE", "missing Stripe-Signature"))?;
        warptalk_stripe::verify_signature(
            raw,
            header,
            secret,
            now.timestamp(),
            self.cfg.webhook_tolerance_secs,
        )
        .map_err(|e| {
            tracing::warn!(error = %e, "stripe webhook rejected");
            AppError::validation("INVALID_SIGNATURE", "invalid webhook signature")
        })?;
        let event = warptalk_stripe::parse_event(raw)
            .map_err(|_| AppError::validation("INVALID_EVENT", "malformed event"))?;
        let effect = self.effect_of(&event).await?;
        let out = self
            .store
            .apply_webhook(&event.id, &event.event_type, effect, now)
            .await?;
        if let Some((ws, state)) = &out.state {
            self.sink.publish(*ws, state).await;
        }
        Ok(WebhookAck {
            received: true,
            duplicate: out.duplicate,
        })
    }

    async fn workspace_from(
        &self,
        obj: &Value,
        customer: Option<&str>,
        stripe_sub: Option<&str>,
    ) -> AppResult<Option<WorkspaceId>> {
        let meta = |p: &str| {
            str_at(obj, p)
                .and_then(|s| s.parse::<WorkspaceId>().ok())
        };
        if let Some(w) = meta("/metadata/workspace_id").or_else(|| meta("/client_reference_id")) {
            return Ok(Some(w));
        }
        if let Some(c) = customer
            && let Some(w) = self.store.workspace_for_customer(c).await?
        {
            return Ok(Some(w));
        }
        if let Some(s) = stripe_sub {
            return self.store.workspace_for_stripe_subscription(s).await;
        }
        Ok(None)
    }

    async fn plan_for_price(&self, price: &str) -> AppResult<Option<Uuid>> {
        Ok(self
            .store
            .list_plans(false)
            .await?
            .into_iter()
            .find(|p| self.price_id(p).as_deref() == Some(price))
            .map(|p| p.id))
    }

    async fn effect_of(&self, ev: &warptalk_stripe::Event) -> AppResult<WebhookEffect> {
        let o = &ev.data.object;
        let customer = str_at(o, "/customer");
        let ignore = |why: &str| {
            tracing::warn!(event = %ev.id, kind = %ev.event_type, why, "stripe event not applicable");
            Ok(WebhookEffect::Ignore)
        };
        match ev.event_type.as_str() {
            "checkout.session.completed" => {
                let paid = matches!(
                    str_at(o, "/payment_status"),
                    Some("paid" | "no_payment_required")
                );
                let session_id = str_at(o, "/id").unwrap_or_default().to_owned();
                let Some(ws) = self.workspace_from(o, customer, None).await? else {
                    return ignore("workspace unknown");
                };
                if !paid {
                    return ignore("session not paid");
                }
                match str_at(o, "/mode") {
                    Some("subscription") => {
                        let Some(plan_id) =
                            str_at(o, "/metadata/plan_id").and_then(|s| s.parse::<Uuid>().ok())
                        else {
                            return ignore("no plan in metadata");
                        };
                        let Some(sub_id) = str_at(o, "/subscription") else {
                            return ignore("no subscription id");
                        };
                        // The period is not on the session; ask Stripe (verified read, not a
                        // state change). Falls back to defaults if unavailable.
                        let (status, period) = match &self.gateway {
                            Some(g) => match g.subscription(sub_id).await {
                                Ok(s) => (
                                    s.status.clone(),
                                    s.period().and_then(|(a, b)| period_of(Some(a), Some(b))),
                                ),
                                Err(e) => {
                                    tracing::warn!(error = %e, "could not retrieve subscription");
                                    ("active".to_owned(), None)
                                }
                            },
                            None => ("active".to_owned(), None),
                        };
                        Ok(WebhookEffect::CheckoutSubscription {
                            ws,
                            plan_id,
                            stripe_subscription_id: sub_id.to_owned(),
                            customer: customer.map(str::to_owned),
                            status,
                            period,
                            session_id,
                        })
                    }
                    Some("payment") => {
                        if str_at(o, "/metadata/kind") != Some("top_up") {
                            return ignore("payment is not a top-up");
                        }
                        let credits = str_at(o, "/metadata/top_up_credits")
                            .and_then(|s| s.parse::<i64>().ok())
                            .filter(|c| *c > 0 && *c <= i64::from(i32::MAX));
                        let Some(credits) = credits else {
                            return ignore("bad top-up credits");
                        };
                        Ok(WebhookEffect::TopUp {
                            ws,
                            credits: credits as i32,
                            session_id,
                            amount_cents: o.pointer("/amount_total").and_then(Value::as_i64).unwrap_or(0),
                            currency: str_at(o, "/currency").unwrap_or("usd").to_owned(),
                        })
                    }
                    _ => ignore("unsupported mode"),
                }
            }
            "invoice.paid" | "invoice.payment_succeeded" => {
                // UNVERIFIED: newer API versions moved `subscription` under
                // `parent.subscription_details.subscription` and the price under
                // `lines.data[].pricing.price_details.price`; both shapes are read.
                let sub_id = str_at(o, "/subscription")
                    .or_else(|| str_at(o, "/parent/subscription_details/subscription"));
                let Some(sub_id) = sub_id else {
                    return ignore("invoice without subscription");
                };
                let Some(ws) = self.workspace_from(o, customer, Some(sub_id)).await? else {
                    return ignore("workspace unknown");
                };
                let price = str_at(o, "/lines/data/0/price/id")
                    .or_else(|| str_at(o, "/lines/data/0/pricing/price_details/price"));
                let plan_id = match price {
                    Some(p) => self.plan_for_price(p).await?,
                    None => None,
                };
                let reason = str_at(o, "/billing_reason").unwrap_or("");
                Ok(WebhookEffect::InvoicePaid {
                    ws,
                    invoice_id: str_at(o, "/id").unwrap_or_default().to_owned(),
                    stripe_subscription_id: Some(sub_id.to_owned()),
                    plan_id,
                    grant_credits: matches!(reason, "subscription_create" | "subscription_cycle"),
                    apply_rollover: reason == "subscription_cycle",
                    period: period_of(
                        o.pointer("/lines/data/0/period/start").and_then(Value::as_i64),
                        o.pointer("/lines/data/0/period/end").and_then(Value::as_i64),
                    ),
                    amount_cents: o.pointer("/amount_paid").and_then(Value::as_i64).unwrap_or(0),
                    currency: str_at(o, "/currency").unwrap_or("usd").to_owned(),
                    hosted_invoice_url: str_at(o, "/hosted_invoice_url").map(str::to_owned),
                    invoice_pdf_url: str_at(o, "/invoice_pdf").map(str::to_owned),
                })
            }
            "invoice.payment_failed" => {
                let sub_id = str_at(o, "/subscription")
                    .or_else(|| str_at(o, "/parent/subscription_details/subscription"));
                let Some(ws) = self.workspace_from(o, customer, sub_id).await? else {
                    return ignore("workspace unknown");
                };
                Ok(WebhookEffect::InvoiceFailed {
                    ws,
                    invoice_id: str_at(o, "/id").unwrap_or_default().to_owned(),
                    amount_cents: o.pointer("/amount_due").and_then(Value::as_i64).unwrap_or(0),
                    currency: str_at(o, "/currency").unwrap_or("usd").to_owned(),
                    reason: Some("payment failed".to_owned()),
                })
            }
            "customer.subscription.updated" | "customer.subscription.deleted" => {
                let Ok(sub) = serde_json::from_value::<warptalk_stripe::Subscription>(o.clone())
                else {
                    return ignore("unreadable subscription");
                };
                let Some(ws) = self.workspace_from(o, Some(&sub.customer), Some(&sub.id)).await?
                else {
                    return ignore("workspace unknown");
                };
                if ev.event_type.ends_with("deleted") {
                    return Ok(WebhookEffect::SubscriptionDeleted {
                        ws,
                        stripe_subscription_id: sub.id,
                    });
                }
                let plan_id = match sub.price_id() {
                    Some(p) => self.plan_for_price(p).await?,
                    None => None,
                };
                Ok(WebhookEffect::SubscriptionUpdated {
                    ws,
                    period: sub.period().and_then(|(a, b)| period_of(Some(a), Some(b))),
                    stripe_subscription_id: sub.id,
                    status: sub.status,
                    cancel_at_period_end: sub.cancel_at_period_end,
                    plan_id,
                })
            }
            _ => Ok(WebhookEffect::Ignore),
        }
    }

    // ---------------------------------------------------------------- admin

    pub async fn admin_set_rate_card(
        &self,
        actor: UserId,
        edits: Vec<RateCardEdit>,
    ) -> AppResult<usize> {
        if edits.is_empty() || edits.len() > 100 {
            return Err(AppError::validation(
                "INVALID_RATE_CARD",
                "send between 1 and 100 entries",
            ));
        }
        let mut inputs = Vec::with_capacity(edits.len());
        for e in edits {
            let feature = Feature::parse(&e.charge_type).ok_or_else(|| {
                AppError::validation("INVALID_RATE_CARD", "unknown chargeType")
            })?;
            let unit_ok = match feature {
                Feature::Stt => e.unit == UNIT_AUDIO_SECOND,
                _ => e.unit == UNIT_TOKENS_IN || e.unit == UNIT_TOKENS_OUT,
            };
            if !unit_ok {
                return Err(AppError::validation(
                    "INVALID_RATE_CARD",
                    format!("unit `{}` does not apply to {}", e.unit, e.charge_type),
                ));
            }
            if !e.unit_price.is_finite() || e.unit_price < 0.0 || e.unit_price > 1_000_000.0 {
                return Err(AppError::validation(
                    "INVALID_RATE_CARD",
                    "unitPrice must be between 0 and 1000000 credits",
                ));
            }
            let model = e
                .model
                .map(|m| m.trim().to_owned())
                .filter(|m| !m.is_empty());
            if model.as_ref().is_some_and(|m| m.chars().count() > 100) {
                return Err(AppError::validation("INVALID_RATE_CARD", "model too long"));
            }
            inputs.push(RateCardInput {
                charge_type: e.charge_type,
                unit: e.unit,
                model,
                unit_price: e.unit_price,
                notes: e.notes,
            });
        }
        self.store
            .replace_rate_card(actor, &inputs, self.clock.now())
            .await?;
        Ok(inputs.len())
    }

    /// Manual wallet adjustment (audited). `idempotency_key` makes a retry a no-op.
    pub async fn admin_adjust(
        &self,
        actor: UserId,
        ws: WorkspaceId,
        amount: i64,
        reason: &str,
        idempotency_key: Option<&str>,
    ) -> AppResult<AdjustResult> {
        let reason = reason.trim();
        if amount == 0 || amount.abs() > 10_000_000 {
            return Err(AppError::validation(
                "INVALID_AMOUNT",
                "amount must be a non-zero integer within +/-10000000",
            ));
        }
        if reason.chars().count() < 3 || reason.chars().count() > 255 {
            return Err(AppError::validation(
                "REASON_REQUIRED",
                "reason must be 3 to 255 characters",
            ));
        }
        let AdjustOutcome {
            duplicate,
            balance,
            state,
        } = self
            .store
            .adjust_credits(
                ws,
                amount as i32,
                reason,
                actor,
                idempotency_key,
                self.clock.now(),
            )
            .await?;
        self.sink.publish(ws, &state).await;
        Ok(AdjustResult {
            balance,
            duplicate,
            service_state: state.as_str().to_owned(),
        })
    }
}
