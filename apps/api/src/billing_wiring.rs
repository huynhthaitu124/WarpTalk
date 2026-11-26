//! Billing composition: adapters between the billing module and its neighbours, and env config.
//!
//! Env: `STRIPE_SECRET_KEY` (absent: checkout/portal answer 503 `STRIPE_NOT_CONFIGURED`),
//! `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_PRO`, `STRIPE_PRICE_BUSINESS`, `BILLING_SUCCESS_URL`,
//! `BILLING_CANCEL_URL`, `BILLING_PORTAL_RETURN_URL` (defaults derive from `APP_BASE_URL`),
//! `BILLING_TOPUP_USD_PER_CREDIT`, `COPILOT_STT_MODEL`.

use std::sync::Arc;

use async_trait::async_trait;
use warptalk_billing::application::ports::WorkspaceAccess;
use warptalk_billing::application::service::BillingConfig;
use warptalk_billing::domain::ServiceState;
use warptalk_billing::stripe::{StripeClient, StripeConfig};
use warptalk_billing::{BillingQueries, infrastructure::stripe_gateway::StripeGateway};
use warptalk_copilot::application::ports::CreditGate;
use warptalk_kernel::{AppResult, UserId, WorkspaceId, WorkspaceRole};
use warptalk_workspace::WorkspaceQueries;

/// Billing's `WorkspaceAccess` port backed by workspace roles.
pub struct BillingWorkspaceAccess(pub WorkspaceQueries);

#[async_trait]
impl WorkspaceAccess for BillingWorkspaceAccess {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        self.0.member_role(ws, user).await
    }
}

/// Copilot's `CreditGate` backed by billing's AI service state. Fails open on read errors.
pub struct BillingCreditGate(pub BillingQueries);

#[async_trait]
impl CreditGate for BillingCreditGate {
    async fn suspended(&self, ws: WorkspaceId) -> Option<String> {
        match self.0.ai_state(ws).await {
            Ok(ServiceState::Suspended(reason)) => Some(reason.to_owned()),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(workspace = %ws, error = %e, "credit gate unreadable; failing open");
                None
            }
        }
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// `STRIPE_SECRET_KEY` -> gateway; `None` when unset or the client cannot be built.
pub fn stripe_gateway_from_env()
-> Option<Arc<dyn warptalk_billing::application::ports::PaymentGateway>> {
    let key = env("STRIPE_SECRET_KEY")?;
    match StripeClient::new(StripeConfig::new(key)) {
        Ok(c) => Some(Arc::new(StripeGateway::new(c))),
        Err(e) => {
            tracing::error!(error = %e, "stripe client unavailable; checkout disabled");
            None
        }
    }
}

pub fn billing_config_from_env() -> BillingConfig {
    let mut cfg = BillingConfig {
        webhook_secret: env("STRIPE_WEBHOOK_SECRET"),
        ..BillingConfig::default()
    };
    for slug in ["pro", "business"] {
        if let Some(p) = env(&format!("STRIPE_PRICE_{}", slug.to_uppercase())) {
            cfg.price_ids.insert(slug.to_owned(), p);
        }
    }
    if let Some(base) = env("APP_BASE_URL") {
        let base = base.trim_end_matches('/');
        cfg.success_url = format!("{base}/billing?checkout=success");
        cfg.cancel_url = format!("{base}/billing?checkout=cancelled");
        cfg.portal_return_url = format!("{base}/billing");
    }
    if let Some(u) = env("BILLING_SUCCESS_URL") {
        cfg.success_url = u;
    }
    if let Some(u) = env("BILLING_CANCEL_URL") {
        cfg.cancel_url = u;
    }
    if let Some(u) = env("BILLING_PORTAL_RETURN_URL") {
        cfg.portal_return_url = u;
    }
    if let Some(p) = env("BILLING_TOPUP_USD_PER_CREDIT")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|p| *p > 0.0)
    {
        cfg.topup_price_per_credit = p;
    }
    cfg
}

/// STT model recorded on copilot usage (`COPILOT_STT_MODEL`, default `gpt-4o-transcribe`).
pub fn copilot_stt_model() -> String {
    env("COPILOT_STT_MODEL").unwrap_or_else(|| "gpt-4o-transcribe".to_owned())
}
