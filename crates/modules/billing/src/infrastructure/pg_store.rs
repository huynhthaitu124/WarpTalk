//! sqlx adapter of [`BillingStore`] (schema `subscription`). Every wallet mutation locks the
//! subscription row (`FOR UPDATE`) and writes the ledger entry with a unique idempotency key, so
//! replays and concurrent writers cannot double-apply (NFR-RL-04, BR-70).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, UserId, WorkspaceId};

use crate::application::ports::{
    AdjustOutcome, BillingStore, GroupBy, PlanRow, RateCardInput, SubscriptionRow, TransactionRow,
    UsageGroup, UsageOutcome, UsageTotals, WebhookEffect, WebhookOutcome,
};
use crate::domain::{
    CURRENCY_CREDITS, Priced, RateCard, RateEntry, ServiceState, UsageEvent, WalletFacts,
    evaluate_state, map_stripe_status,
};

pub struct PgBillingStore {
    pool: PgPool,
}

impl PgBillingStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn db(e: sqlx::Error) -> AppError {
    tracing::error!(error = %e, "billing database error");
    AppError::internal(e.to_string())
}

// ------------------------------------------------------------------ row mapping

#[derive(sqlx::FromRow)]
struct PlanDb {
    id: Uuid,
    name: String,
    slug: String,
    tier: String,
    price: f64,
    currency: String,
    billing_cycle: String,
    credits_per_cycle: i32,
    max_participants: i32,
    max_languages: i32,
    max_active_rooms: i32,
    voice_clone_enabled: bool,
    ai_assistant_enabled: bool,
    glossary_enabled: bool,
    dedicated_gpu: bool,
    overage_cap_credits: i32,
    low_balance_threshold_credits: i32,
    rollover_cap_credits: i32,
    features: Value,
    sort_order: i32,
    is_active: bool,
    stripe_price_id: Option<String>,
}

const PLAN_COLS: &str = "id, name, slug, tier, price::float8 AS price, currency::text AS currency, \
    billing_cycle, credits_per_cycle, max_participants, max_languages, max_active_rooms, \
    voice_clone_enabled, ai_assistant_enabled, glossary_enabled, dedicated_gpu, overage_cap_credits, \
    low_balance_threshold_credits, rollover_cap_credits, features, sort_order, is_active, stripe_price_id";

impl From<PlanDb> for PlanRow {
    fn from(p: PlanDb) -> Self {
        Self {
            id: p.id,
            name: p.name,
            slug: p.slug,
            tier: p.tier,
            price: p.price,
            currency: p.currency.trim().to_owned(),
            billing_cycle: p.billing_cycle,
            credits_per_cycle: p.credits_per_cycle,
            max_participants: p.max_participants,
            max_languages: p.max_languages,
            max_active_rooms: p.max_active_rooms,
            voice_clone_enabled: p.voice_clone_enabled,
            ai_assistant_enabled: p.ai_assistant_enabled,
            glossary_enabled: p.glossary_enabled,
            dedicated_gpu: p.dedicated_gpu,
            overage_cap_credits: p.overage_cap_credits,
            low_balance_threshold_credits: p.low_balance_threshold_credits,
            rollover_cap_credits: p.rollover_cap_credits,
            features: p.features,
            sort_order: p.sort_order,
            is_active: p.is_active,
            stripe_price_id: p.stripe_price_id,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SubDb {
    id: Uuid,
    user_id: Uuid,
    workspace_id: Uuid,
    plan_id: Uuid,
    status: String,
    credits_remaining: i32,
    credits_used_this_cycle: i32,
    current_period_start: DateTime<Utc>,
    current_period_end: DateTime<Utc>,
    auto_renew: bool,
    cancelled_at: Option<DateTime<Utc>>,
    trial_ends_at: Option<DateTime<Utc>>,
    credits_per_cycle_override: Option<i32>,
    overage_cap_credits_override: Option<i32>,
    low_balance_threshold_credits_override: Option<i32>,
    overage_credits_this_cycle: i32,
    service_state: String,
    suspended_reason: Option<String>,
    stripe_subscription_id: Option<String>,
}

const SUB_COLS: &str = "id, user_id, workspace_id, plan_id, status, credits_remaining, \
    credits_used_this_cycle, current_period_start, current_period_end, auto_renew, cancelled_at, \
    trial_ends_at, credits_per_cycle_override, overage_cap_credits_override, \
    low_balance_threshold_credits_override, overage_credits_this_cycle, service_state, \
    suspended_reason, stripe_subscription_id";

impl From<SubDb> for SubscriptionRow {
    fn from(s: SubDb) -> Self {
        Self {
            id: s.id,
            user_id: s.user_id,
            workspace_id: s.workspace_id,
            plan_id: s.plan_id,
            status: s.status,
            credits_remaining: s.credits_remaining,
            credits_used_this_cycle: s.credits_used_this_cycle,
            current_period_start: s.current_period_start,
            current_period_end: s.current_period_end,
            auto_renew: s.auto_renew,
            cancelled_at: s.cancelled_at,
            trial_ends_at: s.trial_ends_at,
            credits_per_cycle_override: s.credits_per_cycle_override,
            overage_cap_override: s.overage_cap_credits_override,
            low_balance_override: s.low_balance_threshold_credits_override,
            overage_credits_this_cycle: s.overage_credits_this_cycle,
            service_state: s.service_state,
            suspended_reason: s.suspended_reason,
            stripe_subscription_id: s.stripe_subscription_id,
        }
    }
}

async fn fetch_plan(c: &mut PgConnection, id: Uuid) -> AppResult<PlanRow> {
    sqlx::query_as::<_, PlanDb>(&format!(
        "SELECT {PLAN_COLS} FROM subscription.plans WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(c)
    .await
    .map_err(db)?
    .map(Into::into)
    .ok_or_else(|| AppError::internal("plan of subscription is missing"))
}

async fn fetch_sub(c: &mut PgConnection, ws: Uuid) -> AppResult<Option<SubscriptionRow>> {
    Ok(sqlx::query_as::<_, SubDb>(&format!(
        "SELECT {SUB_COLS} FROM subscription.subscriptions \
         WHERE workspace_id = $1 AND is_active AND deleted_at IS NULL"
    ))
    .bind(ws)
    .fetch_optional(c)
    .await
    .map_err(db)?
    .map(Into::into))
}

/// Lock the row and return its fresh state.
async fn lock_sub(c: &mut PgConnection, id: Uuid) -> AppResult<SubscriptionRow> {
    sqlx::query_as::<_, SubDb>(&format!(
        "SELECT {SUB_COLS} FROM subscription.subscriptions WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_one(c)
    .await
    .map(Into::into)
    .map_err(db)
}

/// Existing subscription, or the free-trial wallet (credits granted exactly once).
async fn ensure_sub(
    c: &mut PgConnection,
    ws: WorkspaceId,
    user: Option<UserId>,
    now: DateTime<Utc>,
) -> AppResult<(SubscriptionRow, PlanRow)> {
    let wsu = ws.as_uuid();
    if let Some(s) = fetch_sub(c, wsu).await? {
        let p = fetch_plan(c, s.plan_id).await?;
        return Ok((s, p));
    }
    let plan: PlanRow = sqlx::query_as::<_, PlanDb>(&format!(
        "SELECT {PLAN_COLS} FROM subscription.plans WHERE slug = 'free' AND deleted_at IS NULL"
    ))
    .fetch_optional(&mut *c)
    .await
    .map_err(db)?
    .map(Into::into)
    .ok_or_else(|| AppError::unavailable("billing plans are not seeded"))?;
    let trial_days = plan
        .features
        .get("trial_days")
        .and_then(Value::as_i64)
        .unwrap_or(14);
    let id = Uuid::now_v7();
    let owner = user.map(|u| u.as_uuid()).unwrap_or(Uuid::nil());
    let inserted = sqlx::query(
        "INSERT INTO subscription.subscriptions \
         (id, user_id, workspace_id, plan_id, status, current_period_start, current_period_end, \
          trial_ends_at, created_at, created_by) \
         VALUES ($1, $2, $3, $4, 'trialing', $5, $6, $6, $5, $2) \
         ON CONFLICT (workspace_id) WHERE is_active AND workspace_id IS NOT NULL DO NOTHING",
    )
    .bind(id)
    .bind(owner)
    .bind(wsu)
    .bind(plan.id)
    .bind(now)
    .bind(now + Duration::days(trial_days))
    .execute(&mut *c)
    .await
    .map_err(db)?
    .rows_affected();
    if inserted == 1 {
        post_tx(
            c,
            id,
            wsu,
            &TxSpec {
                amount: plan.credits_per_cycle,
                ty: "plan_grant",
                description: "Free trial credits",
                idempotency_key: Some(format!("trial:{wsu}")),
                user_id: owner,
                ..TxSpec::default()
            },
            now,
        )
        .await?;
        persist_state(c, id, now).await?;
    }
    let s = fetch_sub(c, wsu)
        .await?
        .ok_or_else(|| AppError::internal("subscription vanished"))?;
    let p = fetch_plan(c, s.plan_id).await?;
    Ok((s, p))
}

// ------------------------------------------------------------------ ledger

#[derive(Default)]
struct TxSpec {
    amount: i32,
    ty: &'static str,
    charge_type: Option<String>,
    description: &'static str,
    idempotency_key: Option<String>,
    user_id: Uuid,
    usage_record_id: Option<Uuid>,
    rate_id: Option<Uuid>,
    unit_price: Option<f64>,
    reference_id: Option<Uuid>,
    reference_type: Option<&'static str>,
    segment_id: Option<Uuid>,
    description_owned: Option<String>,
}

/// Apply one ledger entry. The subscription row is locked first, so `balance_after` is exact.
/// `Ok(None)`: the idempotency key was already applied, nothing changed.
async fn post_tx(
    c: &mut PgConnection,
    sub_id: Uuid,
    ws: Uuid,
    spec: &TxSpec,
    now: DateTime<Utc>,
) -> AppResult<Option<i32>> {
    let (balance,): (i32,) = sqlx::query_as(
        "SELECT credits_remaining FROM subscription.subscriptions WHERE id = $1 FOR UPDATE",
    )
    .bind(sub_id)
    .fetch_one(&mut *c)
    .await
    .map_err(db)?;
    let new_balance = balance.saturating_add(spec.amount);
    let description = spec
        .description_owned
        .clone()
        .unwrap_or_else(|| spec.description.to_owned());
    let inserted = sqlx::query(
        "INSERT INTO subscription.credit_transactions \
         (id, subscription_id, user_id, workspace_id, amount, type, charge_type, description, \
          reference_id, reference_type, balance_after, pricing_rate_card_id, usage_record_id, \
          unit_price_snapshot, currency, idempotency_key, transcript_segment_id, created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14::float8,$15,$16,$17,$18) \
         ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(sub_id)
    .bind(spec.user_id)
    .bind(ws)
    .bind(spec.amount)
    .bind(spec.ty)
    .bind(&spec.charge_type)
    .bind(description)
    .bind(spec.reference_id)
    .bind(spec.reference_type)
    .bind(new_balance)
    .bind(spec.rate_id)
    .bind(spec.usage_record_id)
    .bind(spec.unit_price)
    .bind(CURRENCY_CREDITS)
    .bind(&spec.idempotency_key)
    .bind(spec.segment_id)
    .bind(now)
    .execute(&mut *c)
    .await
    .map_err(db)?
    .rows_affected();
    if inserted == 0 {
        return Ok(None);
    }
    let used = i32::try_from((-i64::from(spec.amount)).max(0)).unwrap_or(0);
    sqlx::query(
        "UPDATE subscription.subscriptions SET credits_remaining = $2, \
         credits_used_this_cycle = credits_used_this_cycle + $3, updated_at = $4 WHERE id = $1",
    )
    .bind(sub_id)
    .bind(new_balance)
    .bind(used)
    .bind(now)
    .execute(&mut *c)
    .await
    .map_err(db)?;
    Ok(Some(new_balance))
}

/// Re-evaluate the service state from the wallet and persist it (also overage bookkeeping).
async fn persist_state(
    c: &mut PgConnection,
    sub_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<ServiceState> {
    let s = lock_sub(c, sub_id).await?;
    let plan = fetch_plan(c, s.plan_id).await?;
    let state = evaluate_state(&WalletFacts {
        balance: s.credits_remaining,
        status: s.status.clone(),
        trial_ends_at: s.trial_ends_at,
        overage_cap: s.overage_cap(&plan),
        low_threshold: s.low_threshold(&plan),
        now,
    });
    let overage = (-i64::from(s.credits_remaining)).max(0);
    let overage = i32::try_from(overage).unwrap_or(i32::MAX);
    sqlx::query(
        "UPDATE subscription.subscriptions SET service_state = $2, suspended_reason = $3, \
         overage_credits_this_cycle = $4, \
         overage_started_at = CASE WHEN $4 > 0 THEN COALESCE(overage_started_at, $5) ELSE NULL END, \
         updated_at = $5 WHERE id = $1",
    )
    .bind(sub_id)
    .bind(state.as_str())
    .bind(state.suspended_reason())
    .bind(overage)
    .bind(now)
    .execute(c)
    .await
    .map_err(db)?;
    Ok(state)
}

async fn snapshot(c: &mut PgConnection, sub_id: Uuid, now: DateTime<Utc>) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO subscription.credit_balance_snapshots \
         (id, subscription_id, credits_remaining, credits_used_this_cycle, snapshot_at) \
         SELECT $1, id, credits_remaining, credits_used_this_cycle, $3 \
         FROM subscription.subscriptions WHERE id = $2",
    )
    .bind(Uuid::now_v7())
    .bind(sub_id)
    .bind(now)
    .execute(c)
    .await
    .map_err(db)?;
    Ok(())
}

fn money(cents: i64) -> f64 {
    cents as f64 / 100.0
}

#[allow(clippy::too_many_arguments)]
async fn upsert_payment(
    c: &mut PgConnection,
    sub: &SubscriptionRow,
    amount_cents: i64,
    currency: &str,
    method: &str,
    status: &str,
    provider_tx: &str,
    failure: Option<&str>,
    now: DateTime<Utc>,
) -> AppResult<Uuid> {
    let paid_at = (status == "paid").then_some(now);
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO subscription.payments \
         (id, subscription_id, user_id, amount, total_amount, currency, payment_method, provider, \
          provider_transaction_id, status, failure_reason, paid_at, created_at, updated_at) \
         VALUES ($1,$2,$3,$4::float8,$4::float8,$5,$6,'stripe',$7,$8,$9,$10,$11,$11) \
         ON CONFLICT (provider_transaction_id) DO UPDATE SET status = EXCLUDED.status, \
            failure_reason = EXCLUDED.failure_reason, \
            paid_at = COALESCE(EXCLUDED.paid_at, subscription.payments.paid_at), \
            updated_at = EXCLUDED.updated_at \
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(sub.id)
    .bind(sub.user_id)
    .bind(money(amount_cents))
    .bind(currency.to_uppercase().chars().take(3).collect::<String>())
    .bind(method)
    .bind(provider_tx)
    .bind(status)
    .bind(failure.map(|f| f.chars().take(500).collect::<String>()))
    .bind(paid_at)
    .bind(now)
    .fetch_one(c)
    .await
    .map_err(db)?;
    Ok(id)
}

// ------------------------------------------------------------------ store

#[async_trait]
impl BillingStore for PgBillingStore {
    async fn list_plans(&self, active_only: bool) -> AppResult<Vec<PlanRow>> {
        let rows = sqlx::query_as::<_, PlanDb>(&format!(
            "SELECT {PLAN_COLS} FROM subscription.plans \
             WHERE deleted_at IS NULL AND ($1 = false OR is_active) ORDER BY sort_order, name"
        ))
        .bind(active_only)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn plan_by_id(&self, id: Uuid) -> AppResult<Option<PlanRow>> {
        Ok(sqlx::query_as::<_, PlanDb>(&format!(
            "SELECT {PLAN_COLS} FROM subscription.plans WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .map(Into::into))
    }

    async fn plan_by_slug(&self, slug: &str) -> AppResult<Option<PlanRow>> {
        Ok(sqlx::query_as::<_, PlanDb>(&format!(
            "SELECT {PLAN_COLS} FROM subscription.plans WHERE slug = $1 AND deleted_at IS NULL"
        ))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .map(Into::into))
    }

    async fn ensure_subscription(
        &self,
        ws: WorkspaceId,
        user: Option<UserId>,
        now: DateTime<Utc>,
    ) -> AppResult<(SubscriptionRow, PlanRow)> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let out = ensure_sub(&mut tx, ws, user, now).await?;
        tx.commit().await.map_err(db)?;
        Ok(out)
    }

    async fn customer_id(&self, ws: WorkspaceId) -> AppResult<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as(
            "SELECT stripe_customer_id FROM subscription.customers WHERE workspace_id = $1",
        )
        .bind(ws.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(r.map(|r| r.0))
    }

    async fn save_customer(&self, ws: WorkspaceId, customer: &str) -> AppResult<String> {
        sqlx::query(
            "INSERT INTO subscription.customers (workspace_id, stripe_customer_id) VALUES ($1, $2) \
             ON CONFLICT (workspace_id) DO NOTHING",
        )
        .bind(ws.as_uuid())
        .bind(customer)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        self.customer_id(ws)
            .await?
            .ok_or_else(|| AppError::internal("customer not stored"))
    }

    async fn workspace_for_customer(&self, customer: &str) -> AppResult<Option<WorkspaceId>> {
        let r: Option<(Uuid,)> = sqlx::query_as(
            "SELECT workspace_id FROM subscription.customers WHERE stripe_customer_id = $1",
        )
        .bind(customer)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(r.map(|r| WorkspaceId::from_uuid(r.0)))
    }

    async fn workspace_for_stripe_subscription(&self, id: &str) -> AppResult<Option<WorkspaceId>> {
        let r: Option<(Uuid,)> = sqlx::query_as(
            "SELECT workspace_id FROM subscription.subscriptions \
             WHERE stripe_subscription_id = $1 AND workspace_id IS NOT NULL LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(r.map(|r| WorkspaceId::from_uuid(r.0)))
    }

    async fn overrides(&self, ws: WorkspaceId) -> AppResult<HashMap<String, String>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT entitlement_key, value FROM subscription.workspace_entitlement_overrides \
             WHERE workspace_id = $1",
        )
        .bind(ws.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().collect())
    }

    async fn usage_totals(
        &self,
        ws: WorkspaceId,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AppResult<UsageTotals> {
        let (credits, audio, tokens, events): (i64, f64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(credits_consumed), 0)::bigint, \
                    COALESCE(SUM(quantity) FILTER (WHERE unit = 'audio_second'), 0)::float8, \
                    COALESCE(SUM(quantity) FILTER (WHERE unit = 'token'), 0)::bigint, \
                    COUNT(*)::bigint \
             FROM subscription.usage_records \
             WHERE workspace_id = $1 AND recorded_at >= $2 AND recorded_at < $3",
        )
        .bind(ws.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(UsageTotals {
            credits,
            audio_seconds: audio,
            tokens,
            events,
        })
    }

    async fn usage_grouped(
        &self,
        ws: WorkspaceId,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        by: GroupBy,
    ) -> AppResult<Vec<UsageGroup>> {
        let key = match by {
            GroupBy::Day => "to_char(recorded_at AT TIME ZONE 'UTC', 'YYYY-MM-DD')",
            GroupBy::Feature => "usage_type",
            GroupBy::Member => "COALESCE(user_id::text, 'unattributed')",
        };
        let sql = format!(
            "SELECT {key} AS k, COALESCE(SUM(credits_consumed), 0)::bigint, \
                    COALESCE(SUM(quantity), 0)::float8, COUNT(*)::bigint \
             FROM subscription.usage_records \
             WHERE workspace_id = $1 AND recorded_at >= $2 AND recorded_at < $3 \
             GROUP BY k ORDER BY k"
        );
        let rows: Vec<(String, i64, f64, i64)> = sqlx::query_as(&sql)
            .bind(ws.as_uuid())
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|(key, credits, quantity, events)| UsageGroup {
                key,
                credits,
                quantity,
                events,
            })
            .collect())
    }

    async fn transactions(
        &self,
        ws: WorkspaceId,
        limit: i64,
        offset: i64,
    ) -> AppResult<(Vec<TransactionRow>, i64)> {
        #[derive(sqlx::FromRow)]
        struct TxDb {
            id: Uuid,
            amount: i32,
            #[sqlx(rename = "type")]
            ty: String,
            charge_type: Option<String>,
            description: Option<String>,
            balance_after: i32,
            reference_id: Option<Uuid>,
            reference_type: Option<String>,
            created_at: DateTime<Utc>,
        }
        let rows = sqlx::query_as::<_, TxDb>(
            "SELECT id, amount, type, charge_type, description, balance_after, reference_id, \
                    reference_type, created_at \
             FROM subscription.credit_transactions WHERE workspace_id = $1 \
             ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
        )
        .bind(ws.as_uuid())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::bigint FROM subscription.credit_transactions WHERE workspace_id = $1",
        )
        .bind(ws.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok((
            rows.into_iter()
                .map(|r| TransactionRow {
                    id: r.id,
                    amount: r.amount,
                    tx_type: r.ty,
                    charge_type: r.charge_type,
                    description: r.description,
                    balance_after: r.balance_after,
                    reference_id: r.reference_id,
                    reference_type: r.reference_type,
                    created_at: r.created_at,
                })
                .collect(),
            total,
        ))
    }

    async fn rate_card(&self) -> AppResult<RateCard> {
        let rows: Vec<(Uuid, String, Option<String>, Option<String>, f64)> = sqlx::query_as(
            "SELECT id, charge_type, unit, model, unit_price::float8 \
             FROM subscription.usage_rate_card \
             WHERE is_active AND effective_to IS NULL AND currency = $1",
        )
        .bind(CURRENCY_CREDITS)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(RateCard {
            entries: rows
                .into_iter()
                .filter_map(|(id, charge_type, unit, model, unit_price)| {
                    Some(RateEntry {
                        id,
                        charge_type,
                        unit: unit?,
                        model,
                        unit_price,
                    })
                })
                .collect(),
        })
    }

    async fn replace_rate_card(
        &self,
        actor: UserId,
        entries: &[RateCardInput],
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        for e in entries {
            sqlx::query(
                "UPDATE subscription.usage_rate_card SET effective_to = $4, is_active = false \
                 WHERE is_active AND effective_to IS NULL AND currency = $5 AND charge_type = $1 \
                   AND unit = $2 AND model IS NOT DISTINCT FROM $3 AND provider IS NULL",
            )
            .bind(&e.charge_type)
            .bind(&e.unit)
            .bind(&e.model)
            .bind(now)
            .bind(CURRENCY_CREDITS)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            sqlx::query(
                "INSERT INTO subscription.usage_rate_card \
                 (id, charge_type, unit, model, unit_price, currency, effective_from, notes) \
                 VALUES ($1, $2, $3, $4, $5::float8, $6, $7, $8)",
            )
            .bind(Uuid::now_v7())
            .bind(&e.charge_type)
            .bind(&e.unit)
            .bind(&e.model)
            .bind(e.unit_price)
            .bind(CURRENCY_CREDITS)
            .bind(now)
            .bind(&e.notes)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        let details: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({"chargeType": e.charge_type, "unit": e.unit, "model": e.model,
                       "unitPrice": e.unit_price})
            })
            .collect();
        sqlx::query(
            "INSERT INTO subscription.admin_audit (id, actor_id, action, details, created_at) \
             VALUES ($1, $2, 'rate_card.replace', $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(actor.as_uuid())
        .bind(json!({ "entries": details }))
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn record_usage(
        &self,
        e: &UsageEvent,
        priced: &Priced,
        now: DateTime<Utc>,
    ) -> AppResult<UsageOutcome> {
        let ws = WorkspaceId::from_uuid(e.workspace_id);
        let mut tx = self.pool.begin().await.map_err(db)?;
        let (sub, _) = ensure_sub(&mut tx, ws, e.user_id.map(UserId::from_uuid), now).await?;
        // Serialise writers of this wallet before touching it.
        lock_sub(&mut tx, sub.id).await?;
        let (unit, quantity) = e.record_quantity();
        let record_id = Uuid::now_v7();
        let details = json!({
            "feature": e.feature.as_str(), "model": e.model, "audioMs": e.audio_ms,
            "tokensIn": e.tokens_in, "tokensOut": e.tokens_out, "unpriced": priced.unpriced,
        });
        let inserted = sqlx::query(
            "INSERT INTO subscription.usage_records \
             (id, subscription_id, user_id, workspace_id, translation_room_id, segment_id, usage_type, \
              unit, quantity, credits_consumed, duration_seconds, details, recorded_at, event_id, model, \
              cost_source) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9::float8,$10,$11,$12,$13,$14,$15,$16) \
             ON CONFLICT (event_id) WHERE event_id IS NOT NULL DO NOTHING",
        )
        .bind(record_id)
        .bind(sub.id)
        .bind(e.user_id)
        .bind(e.workspace_id)
        .bind(e.room_id)
        .bind(e.segment_id)
        .bind(e.feature.as_str())
        .bind(unit)
        .bind(quantity)
        .bind(priced.credits)
        .bind((e.audio_ms.is_some()).then(|| e.audio_seconds().round() as i32))
        .bind(details)
        .bind(e.occurred_at)
        .bind(&e.event_id)
        .bind(&e.model)
        .bind(e.boundary.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db)?
        .rows_affected();
        if inserted == 0 {
            let state = persist_state(&mut tx, sub.id, now).await?;
            tx.commit().await.map_err(db)?;
            return Ok(UsageOutcome {
                duplicate: true,
                credits: 0,
                state,
            });
        }
        if priced.credits > 0 {
            post_tx(
                &mut tx,
                sub.id,
                e.workspace_id,
                &TxSpec {
                    amount: -priced.credits,
                    ty: "usage",
                    charge_type: Some(e.feature.as_str().to_owned()),
                    description: "AI usage",
                    idempotency_key: Some(format!("usage:{}", e.event_id)),
                    user_id: e.user_id.unwrap_or(sub.user_id),
                    usage_record_id: Some(record_id),
                    rate_id: priced.rate_id,
                    unit_price: priced.unit_price,
                    reference_id: e.room_id,
                    reference_type: e.room_id.map(|_| "room"),
                    segment_id: e.segment_id,
                    ..TxSpec::default()
                },
                now,
            )
            .await?;
        }
        let state = persist_state(&mut tx, sub.id, now).await?;
        tx.commit().await.map_err(db)?;
        Ok(UsageOutcome {
            duplicate: false,
            credits: priced.credits,
            state,
        })
    }

    async fn apply_webhook(
        &self,
        event_id: &str,
        event_type: &str,
        effect: WebhookEffect,
        now: DateTime<Utc>,
    ) -> AppResult<WebhookOutcome> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let fresh = sqlx::query(
            "INSERT INTO subscription.webhook_events (event_id, event_type, received_at) \
             VALUES ($1, $2, $3) ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(event_id)
        .bind(event_type)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?
        .rows_affected();
        if fresh == 0 {
            tx.rollback().await.map_err(db)?;
            return Ok(WebhookOutcome {
                duplicate: true,
                state: None,
            });
        }
        let ws = effect.workspace();
        let state = match effect {
            WebhookEffect::Ignore => None,
            other => Some(apply_effect(&mut tx, other, now).await?),
        };
        tx.commit().await.map_err(db)?;
        Ok(WebhookOutcome {
            duplicate: false,
            state: ws.zip(state),
        })
    }

    async fn adjust_credits(
        &self,
        ws: WorkspaceId,
        amount: i32,
        reason: &str,
        actor: UserId,
        idempotency_key: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<AdjustOutcome> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let (sub, _) = ensure_sub(&mut tx, ws, None, now).await?;
        let applied = post_tx(
            &mut tx,
            sub.id,
            ws.as_uuid(),
            &TxSpec {
                amount,
                ty: "adjustment",
                description: "",
                description_owned: Some(reason.chars().take(255).collect()),
                idempotency_key: idempotency_key.map(|k| format!("adjust:{k}")),
                user_id: actor.as_uuid(),
                reference_id: Some(actor.as_uuid()),
                reference_type: Some("admin"),
                ..TxSpec::default()
            },
            now,
        )
        .await?;
        if applied.is_some() {
            sqlx::query(
                "INSERT INTO subscription.admin_audit (id, actor_id, action, workspace_id, details, created_at) \
                 VALUES ($1, $2, 'credits.adjust', $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(actor.as_uuid())
            .bind(ws.as_uuid())
            .bind(json!({"amount": amount, "reason": reason, "idempotencyKey": idempotency_key}))
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            snapshot(&mut tx, sub.id, now).await?;
        }
        let state = persist_state(&mut tx, sub.id, now).await?;
        let fresh = lock_sub(&mut tx, sub.id).await?;
        tx.commit().await.map_err(db)?;
        Ok(AdjustOutcome {
            duplicate: applied.is_none(),
            balance: fresh.credits_remaining,
            state,
        })
    }

    async fn refresh_state(&self, ws: WorkspaceId, now: DateTime<Utc>) -> AppResult<ServiceState> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let (sub, _) = ensure_sub(&mut tx, ws, None, now).await?;
        let state = persist_state(&mut tx, sub.id, now).await?;
        tx.commit().await.map_err(db)?;
        Ok(state)
    }
}

// ------------------------------------------------------------------ webhook effects

async fn apply_effect(
    c: &mut PgConnection,
    effect: WebhookEffect,
    now: DateTime<Utc>,
) -> AppResult<ServiceState> {
    match effect {
        WebhookEffect::Ignore => Err(AppError::internal("ignore has no state")),
        WebhookEffect::CheckoutSubscription {
            ws,
            plan_id,
            stripe_subscription_id,
            customer,
            status,
            period,
            session_id,
        } => {
            let (sub, _) = ensure_sub(c, ws, None, now).await?;
            lock_sub(c, sub.id).await?;
            let (start, end) = period.unwrap_or((now, now + Duration::days(31)));
            sqlx::query(
                "UPDATE subscription.subscriptions SET plan_id = $2, stripe_subscription_id = $3, \
                 status = $4, current_period_start = $5, current_period_end = $6, auto_renew = true, \
                 cancelled_at = NULL, trial_ends_at = NULL, updated_at = $7 WHERE id = $1",
            )
            .bind(sub.id)
            .bind(plan_id)
            .bind(&stripe_subscription_id)
            .bind(map_stripe_status(&status))
            .bind(start)
            .bind(end)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            if let Some(cus) = customer {
                sqlx::query(
                    "INSERT INTO subscription.customers (workspace_id, stripe_customer_id) \
                     VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(ws.as_uuid())
                .bind(cus)
                .execute(&mut *c)
                .await
                .map_err(db)?;
            }
            tracing::info!(workspace_id = %ws, %session_id, "subscription activated");
            persist_state(c, sub.id, now).await
        }
        WebhookEffect::TopUp {
            ws,
            credits,
            session_id,
            amount_cents,
            currency,
        } => {
            let (sub, _) = ensure_sub(c, ws, None, now).await?;
            let payment = upsert_payment(
                c,
                &sub,
                amount_cents,
                &currency,
                "card",
                "paid",
                &session_id,
                None,
                now,
            )
            .await?;
            post_tx(
                c,
                sub.id,
                ws.as_uuid(),
                &TxSpec {
                    amount: credits,
                    ty: "topup",
                    description: "Credit top-up",
                    idempotency_key: Some(format!("topup:{session_id}")),
                    user_id: sub.user_id,
                    reference_id: Some(payment),
                    reference_type: Some("payment"),
                    ..TxSpec::default()
                },
                now,
            )
            .await?;
            snapshot(c, sub.id, now).await?;
            persist_state(c, sub.id, now).await
        }
        WebhookEffect::InvoicePaid {
            ws,
            invoice_id,
            stripe_subscription_id,
            plan_id,
            grant_credits,
            apply_rollover,
            period,
            amount_cents,
            currency,
            hosted_invoice_url,
            invoice_pdf_url,
        } => {
            let (sub, mut plan) = ensure_sub(c, ws, None, now).await?;
            lock_sub(c, sub.id).await?;
            if let Some(p) = plan_id {
                plan = fetch_plan(c, p).await?;
            }
            let (start, end) = period.unwrap_or((now, now + Duration::days(31)));
            sqlx::query(
                "UPDATE subscription.subscriptions SET plan_id = $2, status = 'active', \
                 stripe_subscription_id = COALESCE($3, stripe_subscription_id), \
                 current_period_start = $4, current_period_end = $5, trial_ends_at = NULL, \
                 updated_at = $6 WHERE id = $1",
            )
            .bind(sub.id)
            .bind(plan.id)
            .bind(&stripe_subscription_id)
            .bind(start)
            .bind(end)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            let payment = upsert_payment(
                c,
                &sub,
                amount_cents,
                &currency,
                "card",
                "paid",
                &invoice_id,
                None,
                now,
            )
            .await?;
            let number = format!(
                "INV-{}-{}",
                now.format("%Y%m"),
                Uuid::now_v7().simple().to_string()[20..28].to_uppercase()
            );
            sqlx::query(
                "INSERT INTO subscription.invoices \
                 (id, payment_id, user_id, workspace_id, invoice_number, stripe_invoice_id, amount, subtotal, \
                  total, currency, status, invoice_pdf_url, hosted_invoice_url, issued_at, due_at, paid_at, created_at) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7::float8,$7::float8,$7::float8,$8,'paid',$9,$10,$11,$11,$11,$11) \
                 ON CONFLICT (stripe_invoice_id) WHERE stripe_invoice_id IS NOT NULL DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(payment)
            .bind(sub.user_id)
            .bind(ws.as_uuid())
            .bind(number)
            .bind(&invoice_id)
            .bind(money(amount_cents))
            .bind(currency.to_uppercase().chars().take(3).collect::<String>())
            .bind(invoice_pdf_url)
            .bind(hosted_invoice_url)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            if grant_credits {
                let balance = lock_sub(c, sub.id).await?.credits_remaining;
                // Rollover (plan data): what exceeds the cap expires before the new grant.
                let carry = balance.clamp(0, plan.rollover_cap_credits.max(0));
                if apply_rollover && balance > carry {
                    post_tx(
                        c,
                        sub.id,
                        ws.as_uuid(),
                        &TxSpec {
                            amount: -(balance - carry),
                            ty: "expiry",
                            description: "Unused credits above the rollover cap",
                            idempotency_key: Some(format!("expire:{invoice_id}")),
                            user_id: sub.user_id,
                            reference_type: Some("invoice"),
                            ..TxSpec::default()
                        },
                        now,
                    )
                    .await?;
                }
                let granted = post_tx(
                    c,
                    sub.id,
                    ws.as_uuid(),
                    &TxSpec {
                        amount: sub.credits_per_cycle(&plan),
                        ty: "plan_grant",
                        description: "Monthly plan credits",
                        idempotency_key: Some(format!("grant:{invoice_id}")),
                        user_id: sub.user_id,
                        reference_type: Some("invoice"),
                        ..TxSpec::default()
                    },
                    now,
                )
                .await?;
                if granted.is_some() {
                    sqlx::query(
                        "UPDATE subscription.subscriptions SET credits_used_this_cycle = 0, \
                         overage_credits_this_cycle = 0 WHERE id = $1",
                    )
                    .bind(sub.id)
                    .execute(&mut *c)
                    .await
                    .map_err(db)?;
                    snapshot(c, sub.id, now).await?;
                }
            }
            persist_state(c, sub.id, now).await
        }
        WebhookEffect::InvoiceFailed {
            ws,
            invoice_id,
            amount_cents,
            currency,
            reason,
        } => {
            let (sub, _) = ensure_sub(c, ws, None, now).await?;
            upsert_payment(
                c,
                &sub,
                amount_cents,
                &currency,
                "card",
                "failed",
                &invoice_id,
                reason.as_deref(),
                now,
            )
            .await?;
            lock_sub(c, sub.id).await?;
            sqlx::query(
                "UPDATE subscription.subscriptions SET status = 'past_due', updated_at = $2 \
                 WHERE id = $1 AND status = 'active'",
            )
            .bind(sub.id)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            persist_state(c, sub.id, now).await
        }
        WebhookEffect::SubscriptionUpdated {
            ws,
            stripe_subscription_id,
            status,
            cancel_at_period_end,
            period,
            plan_id,
        } => {
            let (sub, _) = ensure_sub(c, ws, None, now).await?;
            lock_sub(c, sub.id).await?;
            let mapped = map_stripe_status(&status);
            sqlx::query(
                "UPDATE subscription.subscriptions SET status = $2, auto_renew = $3, \
                 plan_id = COALESCE($4, plan_id), \
                 current_period_start = COALESCE($5, current_period_start), \
                 current_period_end = COALESCE($6, current_period_end), \
                 stripe_subscription_id = $7, \
                 cancelled_at = CASE WHEN $2 = 'cancelled' THEN COALESCE(cancelled_at, $8) ELSE NULL END, \
                 updated_at = $8 WHERE id = $1",
            )
            .bind(sub.id)
            .bind(mapped)
            .bind(!cancel_at_period_end)
            .bind(plan_id)
            .bind(period.map(|p| p.0))
            .bind(period.map(|p| p.1))
            .bind(&stripe_subscription_id)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            persist_state(c, sub.id, now).await
        }
        WebhookEffect::SubscriptionDeleted {
            ws,
            stripe_subscription_id,
        } => {
            let (sub, _) = ensure_sub(c, ws, None, now).await?;
            lock_sub(c, sub.id).await?;
            // A newer Stripe subscription may already be linked: ignore the old one's deletion.
            sqlx::query(
                "UPDATE subscription.subscriptions SET status = 'cancelled', auto_renew = false, \
                 cancelled_at = COALESCE(cancelled_at, $3), updated_at = $3 \
                 WHERE id = $1 AND (stripe_subscription_id IS NULL OR stripe_subscription_id = $2)",
            )
            .bind(sub.id)
            .bind(&stripe_subscription_id)
            .bind(now)
            .execute(&mut *c)
            .await
            .map_err(db)?;
            persist_state(c, sub.id, now).await
        }
    }
}
