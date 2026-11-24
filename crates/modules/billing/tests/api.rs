#![allow(clippy::unwrap_used)]
//! Billing router + metering against the local Postgres, Stripe mocked with wiremock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_billing::application::ports::{ServiceStateSink, WorkspaceAccess};
use warptalk_billing::application::service::BillingConfig;
use warptalk_billing::domain::{Boundary, Feature, ServiceState, UsageEvent};
use warptalk_billing::infrastructure::stripe_gateway::StripeGateway;
use warptalk_billing::stripe::{StripeClient, StripeConfig, webhook::signature_header};
use warptalk_billing::{BillingDeps, BillingModule, MIGRATIONS};
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_web::JwtKeys;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WHSEC: &str = "whsec_unit_test_secret";

#[derive(Default)]
struct Members(Mutex<HashMap<(WorkspaceId, UserId), WorkspaceRole>>);

#[async_trait]
impl WorkspaceAccess for Members {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(self.0.lock().unwrap().get(&(ws, user)).copied())
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<(WorkspaceId, String, Option<String>)>>);

#[async_trait]
impl ServiceStateSink for Sink {
    async fn publish(&self, ws: WorkspaceId, state: &ServiceState) {
        self.0.lock().unwrap().push((
            ws,
            state.as_str().to_owned(),
            state.suspended_reason().map(str::to_owned),
        ));
    }
}

struct Ctx {
    app: Router,
    keys: JwtKeys,
    pool: PgPool,
    members: Arc<Members>,
    sink: Arc<Sink>,
    module: BillingModule,
    stripe: MockServer,
    ws: WorkspaceId,
    owner: UserId,
    admin: UserId,
    member: UserId,
}

async fn ctx() -> Option<Ctx> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let stripe = MockServer::start().await;
    let client =
        StripeClient::new(StripeConfig::new("sk_test_123").with_base_url(stripe.uri())).unwrap();
    let members = Arc::new(Members::default());
    let sink = Arc::new(Sink::default());
    let mut cfg = BillingConfig {
        webhook_secret: Some(WHSEC.into()),
        ..BillingConfig::default()
    };
    cfg.price_ids.insert("pro".into(), "price_pro_test".into());
    let module = BillingModule::new(BillingDeps {
        pool: pool.clone(),
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        access: members.clone(),
        gateway: Some(Arc::new(StripeGateway::new(client))),
        state_sink: Some(sink.clone()),
        config: cfg,
    });
    let (ws, owner, admin, member) = (
        WorkspaceId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
    );
    {
        let mut m = members.0.lock().unwrap();
        m.insert((ws, owner), WorkspaceRole::Owner);
        m.insert((ws, admin), WorkspaceRole::Admin);
        m.insert((ws, member), WorkspaceRole::Member);
    }
    Some(Ctx {
        app: module.router(),
        keys,
        pool,
        members,
        sink,
        module,
        stripe,
        ws,
        owner,
        admin,
        member,
    })
}

impl Ctx {
    fn token(&self, user: UserId, sa: bool) -> String {
        self.keys
            .issue_access(user, Uuid::now_v7(), sa, Utc::now())
            .unwrap()
            .0
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self, uri: &str, user: UserId) -> (StatusCode, Value) {
        let t = self.token(user, false);
        self.call("GET", uri, Some(&t), None).await
    }

    async fn webhook(&self, event: &Value) -> (StatusCode, Value) {
        let raw = event.to_string();
        let sig = signature_header(raw.as_bytes(), Utc::now().timestamp(), WHSEC);
        self.raw_webhook(raw, Some(&sig)).await
    }

    async fn raw_webhook(&self, raw: String, sig: Option<&str>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri("/billing/webhooks/stripe")
            .header("content-type", "application/json");
        if let Some(s) = sig {
            b = b.header("stripe-signature", s);
        }
        let resp = self
            .app
            .clone()
            .oneshot(b.body(Body::from(raw)).unwrap())
            .await
            .unwrap();
        let st = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn billing(&self) -> Value {
        let (st, v) = self
            .get(&format!("/workspaces/{}/billing", self.ws), self.owner)
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v
    }

    async fn balance(&self) -> i64 {
        self.billing().await["balance"].as_i64().unwrap()
    }

    async fn plan_id(&self, slug: &str) -> Uuid {
        let (_, v) = self.get("/billing/plans", self.owner).await;
        v.as_array()
            .unwrap()
            .iter()
            .find(|p| p["slug"] == slug)
            .map(|p| p["id"].as_str().unwrap().parse().unwrap())
            .unwrap()
    }

    async fn link_customer(&self, cus: &str) {
        sqlx::query(
            "INSERT INTO subscription.customers (workspace_id, stripe_customer_id) VALUES ($1, $2)",
        )
        .bind(self.ws.as_uuid())
        .bind(cus)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn tx_count(&self, like: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM subscription.credit_transactions WHERE workspace_id = $1 AND idempotency_key LIKE $2")
            .bind(self.ws.as_uuid())
            .bind(like)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    fn usage_event(&self, id: &str, feature: Feature, boundary: Boundary) -> UsageEvent {
        UsageEvent {
            event_id: id.to_owned(),
            workspace_id: self.ws.as_uuid(),
            room_id: Some(Uuid::now_v7()),
            user_id: Some(self.owner.as_uuid()),
            feature,
            model: "gpt-test".into(),
            audio_ms: None,
            tokens_in: None,
            tokens_out: None,
            boundary,
            occurred_at: Utc::now(),
            segment_id: Some(Uuid::now_v7()),
        }
    }
}

fn evt(id: &str, ty: &str, obj: Value) -> Value {
    json!({"id": id, "type": ty, "created": Utc::now().timestamp(), "data": {"object": obj}})
}

// ---------------------------------------------------------------- catalog + overview

#[tokio::test]
async fn plans_are_seeded_data_and_overview_creates_the_trial_wallet_once() {
    let Some(c) = ctx().await else { return };
    let (st, plans) = c.get("/billing/plans", c.member).await;
    assert_eq!(st, StatusCode::OK);
    let slugs: Vec<&str> = plans
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap())
        .collect();
    for s in ["free", "pro", "business"] {
        assert!(slugs.contains(&s), "{slugs:?}");
    }
    let pro = plans
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "pro")
        .unwrap();
    assert_eq!(pro["purchasable"], true, "price id comes from config");
    let business = plans
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "business")
        .unwrap();
    assert_eq!(business["purchasable"], false);

    let b = c.billing().await;
    assert_eq!(b["balance"], 1800);
    assert_eq!(b["subscription"]["status"], "trialing");
    assert_eq!(b["serviceState"], "healthy");
    assert_eq!(b["entitlements"]["max_concurrent_copilot_sessions"], 1);
    // asking again does not grant again
    assert_eq!(c.balance().await, 1800);
    assert_eq!(c.tx_count("trial:%").await, 1);

    let (st, _) = c
        .get(&format!("/workspaces/{}/billing", c.ws), UserId::new())
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "non-members see nothing");
}

// ---------------------------------------------------------------- checkout

#[tokio::test]
async fn checkout_creates_customer_and_session_via_stripe_and_is_owner_only() {
    let Some(c) = ctx().await else { return };
    Mock::given(method("POST"))
        .and(path("/v1/customers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "cus_wm_1"})))
        .expect(1)
        .mount(&c.stripe)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/checkout/sessions"))
        .and(body_string_contains("mode=subscription"))
        .and(body_string_contains("price_pro_test"))
        .and(body_string_contains("customer=cus_wm_1"))
        .and(body_string_contains(c.ws.to_string()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "cs_1", "url": "https://checkout.example/cs_1"})),
        )
        .expect(1)
        .mount(&c.stripe)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/checkout/sessions"))
        .and(body_string_contains("mode=payment"))
        .and(body_string_contains("unit_amount%5D=100"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "cs_2", "url": "https://checkout.example/cs_2"})),
        )
        .expect(1)
        .mount(&c.stripe)
        .await;
    let pro = c.plan_id("pro").await;
    let uri = format!("/workspaces/{}/billing/checkout", c.ws);

    for u in [c.member, c.admin] {
        let t = c.token(u, false);
        let (st, v) = c
            .call("POST", &uri, Some(&t), Some(json!({"planId": pro})))
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    }
    let t = c.token(c.owner, false);
    let (st, v) = c
        .call("POST", &uri, Some(&t), Some(json!({"planId": pro})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["url"], "https://checkout.example/cs_1");

    // Free trial cannot top up; neither of both / none is valid
    let (st, v) = c
        .call("POST", &uri, Some(&t), Some(json!({"topUpCredits": 5000})))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (st, _) = c.call("POST", &uri, Some(&t), Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            "POST",
            &uri,
            Some(&t),
            Some(json!({"planId": pro, "topUpCredits": 5000})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // business has no price id configured
    let biz = c.plan_id("business").await;
    let (st, v) = c
        .call("POST", &uri, Some(&t), Some(json!({"planId": biz})))
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["code"], "PRICE_NOT_CONFIGURED");

    // after activation a top-up session is allowed (5000 credits x 0.0002 = $1.00)
    let t0 = c.webhook(&evt("evt_act_a", "checkout.session.completed", json!({
        "id": "cs_1", "mode": "subscription", "payment_status": "paid", "customer": "cus_wm_1",
        "subscription": "sub_wm_1", "metadata": {"workspace_id": c.ws.to_string(), "plan_id": pro.to_string()}
    }))).await;
    assert_eq!(t0.0, StatusCode::OK);
    let (st, v) = c
        .call("POST", &uri, Some(&t), Some(json!({"topUpCredits": 5000})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["url"], "https://checkout.example/cs_2");
    // already subscribed (BR-65)
    let (st, v) = c
        .call("POST", &uri, Some(&t), Some(json!({"planId": pro})))
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "ALREADY_SUBSCRIBED");
}

#[tokio::test]
async fn portal_needs_a_customer_and_the_owner() {
    let Some(c) = ctx().await else { return };
    let uri = format!("/workspaces/{}/billing/portal", c.ws);
    let t = c.token(c.owner, false);
    let (st, v) = c.call("POST", &uri, Some(&t), None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    c.link_customer("cus_portal").await;
    Mock::given(method("POST"))
        .and(path("/v1/billing_portal/sessions"))
        .and(body_string_contains("customer=cus_portal"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "bps", "url": "https://portal.example/p"})),
        )
        .mount(&c.stripe)
        .await;
    let (st, v) = c.call("POST", &uri, Some(&t), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["url"], "https://portal.example/p");
    let tm = c.token(c.member, false);
    let (st, _) = c.call("POST", &uri, Some(&tm), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------- webhook signature

#[tokio::test]
async fn webhook_signature_good_bad_and_replay() {
    let Some(c) = ctx().await else { return };
    let raw = evt("evt_sig_1", "ping.unknown", json!({})).to_string();
    let now = Utc::now().timestamp();

    let good = signature_header(raw.as_bytes(), now, WHSEC);
    let (st, v) = c.raw_webhook(raw.clone(), Some(&good)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["received"], true);

    let bad = signature_header(raw.as_bytes(), now, "whsec_wrong");
    let (st, v) = c.raw_webhook(raw.clone(), Some(&bad)).await;
    assert_eq!(
        (st, v["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("INVALID_SIGNATURE"))
    );
    let (st, _) = c.raw_webhook(raw.clone(), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // tampered body with a valid header for the original
    let (st, _) = c
        .raw_webhook(raw.replace("evt_sig_1", "evt_sig_2"), Some(&good))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // replay of an old capture: outside the 300 s tolerance
    let old = signature_header(raw.as_bytes(), now - 301, WHSEC);
    let (st, _) = c.raw_webhook(raw.clone(), Some(&old)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // the same valid event again is acknowledged as a duplicate, no error
    let (st, v) = c.raw_webhook(raw, Some(&good)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["duplicate"], true);
}

// ---------------------------------------------------------------- webhook-driven state

#[tokio::test]
async fn activation_grant_renewal_failure_and_cancel_are_webhook_driven_and_idempotent() {
    let Some(c) = ctx().await else { return };
    let pro = c.plan_id("pro").await;
    let ws = c.ws.to_string();
    c.link_customer("cus_flow").await;
    Mock::given(method("GET"))
        .and(path("/v1/subscriptions/sub_flow"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "sub_flow", "status": "active", "customer": "cus_flow",
            "current_period_start": 1_800_000_000, "current_period_end": 1_802_592_000,
            "items": {"data": [{"price": {"id": "price_pro_test"}}]}
        })))
        .mount(&c.stripe)
        .await;
    assert_eq!(c.balance().await, 1800);

    // No state change from anything but a verified event: before the webhook it is still a trial.
    assert_eq!(c.billing().await["subscription"]["status"], "trialing");

    let checkout = evt(
        "evt_co_1",
        "checkout.session.completed",
        json!({
            "id": "cs_flow", "mode": "subscription", "payment_status": "paid", "customer": "cus_flow",
            "subscription": "sub_flow", "metadata": {"workspace_id": ws, "plan_id": pro.to_string()}
        }),
    );
    let (st, v) = c.webhook(&checkout).await;
    assert_eq!(
        (st, v["duplicate"].as_bool()),
        (StatusCode::OK, Some(false)),
        "{v}"
    );
    let b = c.billing().await;
    assert_eq!(b["subscription"]["status"], "active");
    assert_eq!(b["subscription"]["plan"]["slug"], "pro");
    assert_eq!(
        b["subscription"]["currentPeriodEnd"].as_str().unwrap()[..4],
        *"2027"
    );
    assert_eq!(b["balance"], 1800, "activation itself grants nothing");

    let invoice = evt(
        "evt_inv_1",
        "invoice.paid",
        json!({
            "id": "in_1", "customer": "cus_flow", "subscription": "sub_flow",
            "billing_reason": "subscription_create", "amount_paid": 2900, "currency": "usd",
            "hosted_invoice_url": "https://inv.example/1",
            "lines": {"data": [{"price": {"id": "price_pro_test"}, "period": {"start": 1_800_000_000, "end": 1_802_592_000}}]}
        }),
    );
    let (st, v) = c.webhook(&invoice).await;
    assert_eq!(
        (st, v["duplicate"].as_bool()),
        (StatusCode::OK, Some(false)),
        "{v}"
    );
    assert_eq!(
        c.balance().await,
        1800 + 36000,
        "trial credits survive the upgrade, plan credits granted"
    );

    // replay of the same event id, and a re-delivery under a new event id for the same invoice
    for id in ["evt_inv_1", "evt_inv_1b"] {
        let mut again = invoice.clone();
        again["id"] = json!(id);
        let (st, v) = c.webhook(&again).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["duplicate"].as_bool(), Some(id == "evt_inv_1"));
    }
    assert_eq!(c.balance().await, 37800, "granted once");
    assert_eq!(c.tx_count("grant:in_1").await, 1);
    let invoices: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM subscription.invoices WHERE workspace_id = $1")
            .bind(c.ws.as_uuid())
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert_eq!(invoices, 1);

    // renewal: Pro has no rollover, so what is left expires before the new grant
    let renewal = evt(
        "evt_inv_2",
        "invoice.paid",
        json!({
            "id": "in_2", "customer": "cus_flow", "subscription": "sub_flow",
            "billing_reason": "subscription_cycle", "amount_paid": 2900, "currency": "usd",
            "lines": {"data": [{"price": {"id": "price_pro_test"}}]}
        }),
    );
    c.webhook(&renewal).await;
    assert_eq!(c.balance().await, 36000);
    assert_eq!(c.tx_count("expire:in_2").await, 1);

    // payment failure -> past_due (grace), still served
    let (st, _) = c.webhook(&evt("evt_fail_1", "invoice.payment_failed", json!({
        "id": "in_3", "customer": "cus_flow", "subscription": "sub_flow", "amount_due": 2900, "currency": "usd"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    let b = c.billing().await;
    assert_eq!(b["subscription"]["status"], "past_due");
    assert_eq!(b["serviceState"], "healthy");

    // scheduled cancellation, then the subscription ends
    c.webhook(&evt("evt_upd_1", "customer.subscription.updated", json!({
        "id": "sub_flow", "status": "active", "customer": "cus_flow", "cancel_at_period_end": true,
        "current_period_start": 1_800_000_000, "current_period_end": 1_802_592_000,
        "items": {"data": [{"price": {"id": "price_pro_test"}}]}
    }))).await;
    let b = c.billing().await;
    assert_eq!(
        (
            b["subscription"]["status"].as_str(),
            b["subscription"]["autoRenew"].as_bool()
        ),
        (Some("active"), Some(false))
    );
    c.webhook(&evt(
        "evt_del_1",
        "customer.subscription.deleted",
        json!({
            "id": "sub_flow", "status": "canceled", "customer": "cus_flow",
            "items": {"data": []}
        }),
    ))
    .await;
    assert_eq!(c.billing().await["subscription"]["status"], "cancelled");
    assert_eq!(
        c.balance().await,
        36000,
        "cancelling keeps the remaining credits"
    );
}

#[tokio::test]
async fn top_up_is_credited_once_even_when_redelivered() {
    let Some(c) = ctx().await else { return };
    c.link_customer("cus_topup").await;
    let ev = |id: &str| {
        evt(
            id,
            "checkout.session.completed",
            json!({
                "id": "cs_topup", "mode": "payment", "payment_status": "paid", "customer": "cus_topup",
                "amount_total": 100, "currency": "usd",
                "metadata": {"workspace_id": c.ws.to_string(), "kind": "top_up", "top_up_credits": "5000"}
            }),
        )
    };
    assert_eq!(c.balance().await, 1800);
    c.webhook(&ev("evt_tu_1")).await;
    c.webhook(&ev("evt_tu_1")).await;
    c.webhook(&ev("evt_tu_2")).await;
    assert_eq!(c.balance().await, 6800);
    assert_eq!(c.tx_count("topup:cs_topup").await, 1);
    // unpaid sessions (async payment methods) change nothing
    let mut unpaid = ev("evt_tu_3");
    unpaid["data"]["object"]["payment_status"] = json!("unpaid");
    unpaid["data"]["object"]["id"] = json!("cs_other");
    c.webhook(&unpaid).await;
    assert_eq!(c.balance().await, 6800);
    let (st, v) = c
        .get(
            &format!("/workspaces/{}/billing/transactions", c.ws),
            c.owner,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"][0]["type"], "topup", "newest first");
    assert_eq!(v["total"], 2);
}

// ---------------------------------------------------------------- metering

#[tokio::test]
async fn usage_is_priced_from_the_rate_card_and_debited_once_per_event() {
    let Some(c) = ctx().await else { return };
    let metering = c.module.metering();
    assert_eq!(c.balance().await, 1800);

    let mut stt = c.usage_event("stt:idem-1", Feature::Stt, Boundary::Platform);
    stt.audio_ms = Some(12_400);
    let out = metering.process(&stt).await.unwrap();
    assert_eq!((out.duplicate, out.credits), (false, 13));
    assert_eq!(c.balance().await, 1800 - 13);

    // same event again (Redis redelivery / worker restart): no second debit, no second record
    let out = metering.process(&stt).await.unwrap();
    assert!(out.duplicate);
    assert_eq!(c.balance().await, 1800 - 13);
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM subscription.usage_records WHERE event_id = 'stt:idem-1'",
    )
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(c.tx_count("usage:stt:idem-1").await, 1);

    // per-segment traceability (RV-01)
    let seg: Option<Uuid> = sqlx::query_scalar(
        "SELECT segment_id FROM subscription.usage_records WHERE event_id = 'stt:idem-1'",
    )
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(seg, stt.segment_id);

    // LLM: 2K in x1 + 1K out x3 = 5
    let mut llm = c.usage_event("llm:idem-2", Feature::LlmSummary, Boundary::Platform);
    llm.tokens_in = Some(2000);
    llm.tokens_out = Some(1000);
    assert_eq!(metering.process(&llm).await.unwrap().credits, 5);
    assert_eq!(c.balance().await, 1800 - 18);

    // report by feature
    let (st, v) = c
        .get(
            &format!("/workspaces/{}/billing/usage?groupBy=feature", c.ws),
            c.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let groups = v["groups"].as_array().unwrap();
    let credits = |k: &str| {
        groups
            .iter()
            .find(|g| g["key"] == k)
            .map(|g| g["credits"].as_i64().unwrap())
    };
    assert_eq!(
        (credits("stt"), credits("llm_summary")),
        (Some(13), Some(5))
    );
    let (st, _) = c
        .get(
            &format!("/workspaces/{}/billing/usage?groupBy=bogus", c.ws),
            c.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .get(&format!("/workspaces/{}/billing/usage", c.ws), c.member)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let b = c.billing().await;
    assert_eq!(b["usage"]["creditsConsumed"], 18);
}

#[tokio::test]
async fn byo_usage_is_recorded_with_zero_cost() {
    let Some(c) = ctx().await else { return };
    let metering = c.module.metering();
    let before = c.balance().await;
    let mut e = c.usage_event("byo:idem-1", Feature::LlmSuggestion, Boundary::Byo);
    e.tokens_in = Some(10_000);
    e.tokens_out = Some(2_000);
    let out = metering.process(&e).await.unwrap();
    assert_eq!((out.duplicate, out.credits), (false, 0));
    assert_eq!(c.balance().await, before);
    let (source, credits): (String, i32) = sqlx::query_as("SELECT cost_source, credits_consumed FROM subscription.usage_records WHERE event_id = 'byo:idem-1'")
        .fetch_one(&c.pool).await.unwrap();
    assert_eq!((source.as_str(), credits), ("byo", 0));
    assert_eq!(
        c.tx_count("usage:byo:idem-1").await,
        0,
        "no ledger entry for a free charge"
    );
}

#[tokio::test]
async fn exhausted_credits_suspend_the_service_and_publish_the_state() {
    let Some(c) = ctx().await else { return };
    let metering = c.module.metering();
    let queries = c.module.queries();
    assert_eq!(queries.ai_state(c.ws).await.unwrap(), ServiceState::Healthy);

    // burn almost everything with one big STT event: 1800 s = 1800 credits
    let mut e = c.usage_event("drain:1", Feature::Stt, Boundary::Platform);
    e.audio_ms = Some(1_800_000);
    let out = metering.process(&e).await.unwrap();
    assert_eq!(out.state, ServiceState::Suspended("insufficient_credits"));
    assert_eq!(
        queries.ai_state(c.ws).await.unwrap(),
        ServiceState::Suspended("insufficient_credits")
    );
    let published = c.sink.0.lock().unwrap().clone();
    assert!(
        published.iter().any(|(w, s, r)| *w == c.ws
            && s == "suspended"
            && r.as_deref() == Some("insufficient_credits")),
        "{published:?}"
    );
    let b = c.billing().await;
    assert_eq!(
        (b["serviceState"].as_str(), b["suspendedReason"].as_str()),
        (Some("suspended"), Some("insufficient_credits"))
    );

    // usage is still recorded (nothing is lost), balance goes negative
    let mut late = c.usage_event("drain:2", Feature::Stt, Boundary::Platform);
    late.audio_ms = Some(5_000);
    metering.process(&late).await.unwrap();
    assert_eq!(c.balance().await, -5);

    // an admin top-up restores service and republishes healthy
    let adm = c.token(c.admin, true);
    let (st, v) = c
        .call(
            "POST",
            &format!("/admin/workspaces/{}/credits/adjust", c.ws),
            Some(&adm),
            Some(json!({"amount": 1000, "reason": "goodwill"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["balance"], 995);
    assert_eq!(queries.ai_state(c.ws).await.unwrap(), ServiceState::Healthy);
    let last = c.sink.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.1, "healthy");
}

// ---------------------------------------------------------------- admin

#[tokio::test]
async fn admin_adjust_is_system_admin_only_audited_and_idempotent() {
    let Some(c) = ctx().await else { return };
    let uri = format!("/admin/workspaces/{}/credits/adjust", c.ws);
    let body =
        json!({"amount": 250, "reason": "support credit INC-42", "idempotencyKey": "inc-42"});
    // workspace owners are not system admins
    let t = c.token(c.owner, false);
    let (st, _) = c.call("POST", &uri, Some(&t), Some(body.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call("POST", &uri, None, Some(body.clone())).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let a = c.token(c.admin, true);
    let (st, v) = c.call("POST", &uri, Some(&a), Some(body.clone())).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["balance"].as_i64(), v["duplicate"].as_bool()),
        (Some(2050), Some(false))
    );
    let (st, v) = c.call("POST", &uri, Some(&a), Some(body)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (v["balance"].as_i64(), v["duplicate"].as_bool()),
        (Some(2050), Some(true))
    );

    for bad in [
        json!({"amount": 0, "reason": "x y z"}),
        json!({"amount": 5, "reason": ""}),
    ] {
        let (st, _) = c.call("POST", &uri, Some(&a), Some(bad)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    let (n, actor): (i64, Uuid) = sqlx::query_as("SELECT COUNT(*), MIN(actor_id::text)::uuid FROM subscription.admin_audit WHERE workspace_id = $1 AND action = 'credits.adjust'")
        .bind(c.ws.as_uuid()).fetch_one(&c.pool).await.unwrap();
    assert_eq!(
        (n, actor),
        (1, c.admin.as_uuid()),
        "one audit row, naming the admin"
    );
    let (st, v) = c
        .get(
            &format!("/workspaces/{}/billing/transactions", c.ws),
            c.owner,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["items"][0]["type"], "adjustment");
    assert_eq!(v["items"][0]["description"], "support credit INC-42");
    let _ = &c.members;
}

#[tokio::test]
async fn rate_card_edit_changes_pricing_and_is_audited() {
    let Some(c) = ctx().await else { return };
    let a = c.token(c.admin, true);
    let t = c.token(c.owner, false);
    let model = format!("stt-model-{}", Uuid::now_v7());
    let body = json!({"entries": [{"chargeType": "stt", "unit": "audio_second", "model": model, "unitPrice": 0.5, "notes": "test"}]});
    let (st, _) = c
        .call(
            "PUT",
            "/admin/billing/rate-card",
            Some(&t),
            Some(body.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v) = c
        .call("PUT", "/admin/billing/rate-card", Some(&a), Some(body))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    for bad in [
        json!({"entries": []}),
        json!({"entries": [{"chargeType": "stt", "unit": "1k_tokens_in", "unitPrice": 1.0}]}),
        json!({"entries": [{"chargeType": "stt", "unit": "audio_second", "unitPrice": -1.0}]}),
        json!({"entries": [{"chargeType": "nope", "unit": "audio_second", "unitPrice": 1.0}]}),
    ] {
        let (st, _) = c
            .call("PUT", "/admin/billing/rate-card", Some(&a), Some(bad))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    let mut e = c.usage_event("rc:1", Feature::Stt, Boundary::Platform);
    e.model = model;
    e.audio_ms = Some(10_000);
    assert_eq!(
        c.module.metering().process(&e).await.unwrap().credits,
        5,
        "model-specific rate wins"
    );
    // a rate edit versions the row: the previous one is closed, not deleted
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription.usage_rate_card WHERE charge_type = 'stt' AND unit = 'audio_second' AND model IS NULL AND is_active AND effective_to IS NULL")
        .fetch_one(&c.pool).await.unwrap();
    assert_eq!(live, 1);
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription.admin_audit WHERE actor_id = $1 AND action = 'rate_card.replace'")
        .bind(c.admin.as_uuid()).fetch_one(&c.pool).await.unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn concurrent_delivery_of_one_event_debits_once() {
    let Some(c) = ctx().await else { return };
    let metering = c.module.metering();
    let mut e = c.usage_event("conc:1", Feature::Stt, Boundary::Platform);
    e.audio_ms = Some(20_000);
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (m, e) = (metering.clone(), e.clone());
        set.spawn(async move { m.process(&e).await.unwrap() });
    }
    let mut applied = 0;
    while let Some(r) = set.join_next().await {
        if !r.unwrap().duplicate {
            applied += 1;
        }
    }
    assert_eq!(applied, 1);
    assert_eq!(c.balance().await, 1800 - 20);
}
