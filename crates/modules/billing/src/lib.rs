//! Plans, subscriptions, Stripe checkout and webhooks, credit wallet, usage metering, invoices (schema subscription).
//!
//! Layers (dependencies point inward):
//! - `domain`: usage events, rate-card pricing, service-state rules, entitlements. No IO.
//! - `application`: use cases ([`application::service::BillingService`],
//!   [`application::metering::MeteringService`]) and ports (store, payment gateway, state sink).
//! - `infrastructure`: sqlx store, Redis state sink, Stripe gateway.
//! - `http`: axum handlers (`/billing/*`, `/workspaces/{id}/billing/*`, `/admin/*`).
//!
//! Metering: producers (copilot, assistant) XADD [`domain::UsageEvent`] JSON to
//! [`domain::USAGE_STREAM`]; the worker feeds each entry to [`BillingModule::metering`].

pub mod application;
pub mod domain;
pub mod http;
pub mod infrastructure;

use std::sync::Arc;

use sqlx::PgPool;
use warptalk_db::{Migration, ModuleMigrations};
use warptalk_kernel::{AppResult, Clock, WorkspaceId};
use warptalk_web::JwtKeys;

use application::metering::MeteringService;
use application::ports::{
    BillingStore, NoStateSink, PaymentGateway, ServiceStateSink, WorkspaceAccess,
};
use application::service::{BillingConfig, BillingService};
use domain::ServiceState;
use infrastructure::pg_store::PgBillingStore;

/// Re-exported so apps need no direct dependency on the provider crate.
pub use warptalk_stripe as stripe;

pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "subscription",
    migrations: &[Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    }],
};

pub struct BillingDeps {
    pub pool: PgPool,
    pub jwt: JwtKeys,
    pub clock: Arc<dyn Clock>,
    pub access: Arc<dyn WorkspaceAccess>,
    /// `None`: checkout and portal answer 503 `STRIPE_NOT_CONFIGURED`.
    pub gateway: Option<Arc<dyn PaymentGateway>>,
    /// Redis keys for the AI service state; `None` writes nothing.
    pub state_sink: Option<Arc<dyn ServiceStateSink>>,
    pub config: BillingConfig,
}

pub struct BillingModule {
    service: Arc<BillingService>,
    metering: Arc<MeteringService>,
    jwt: JwtKeys,
}

impl BillingModule {
    pub fn new(deps: BillingDeps) -> Self {
        let store: Arc<dyn BillingStore> = Arc::new(PgBillingStore::new(deps.pool));
        let sink: Arc<dyn ServiceStateSink> = deps.state_sink.unwrap_or_else(|| Arc::new(NoStateSink));
        Self {
            service: Arc::new(BillingService::new(
                store.clone(),
                deps.access,
                deps.gateway,
                sink.clone(),
                deps.clock.clone(),
                deps.config,
            )),
            metering: Arc::new(MeteringService::new(store, sink, deps.clock)),
            jwt: deps.jwt,
        }
    }

    /// Routes relative to the API prefix (`/api/v1`), state applied.
    pub fn router(&self) -> axum::Router {
        http::router(http::AppState {
            service: self.service.clone(),
            jwt: self.jwt.clone(),
        })
    }

    /// Usage consumer used by the worker.
    pub fn metering(&self) -> Arc<MeteringService> {
        self.metering.clone()
    }

    /// Read facade for other composition-root adapters (credit gate).
    pub fn queries(&self) -> BillingQueries {
        BillingQueries {
            service: self.service.clone(),
        }
    }
}

#[derive(Clone)]
pub struct BillingQueries {
    service: Arc<BillingService>,
}

impl BillingQueries {
    /// Current AI service state of the workspace (`Suspended(reason)` when credits are exhausted).
    pub async fn ai_state(&self, ws: WorkspaceId) -> AppResult<ServiceState> {
        self.service.ai_state(ws).await
    }
}
