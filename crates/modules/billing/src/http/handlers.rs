use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{AppError, Page, PageRequest, WorkspaceId};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{ApiResult, AuthUser, SystemAdmin};

use super::AppState;
use crate::application::service::{CheckoutTarget, RateCardEdit};
use crate::application::views::{
    AdjustResult, BillingOverview, CheckoutUrl, PlanView, TransactionView, UsageReport,
    WebhookAck,
};

pub async fn plans(State(s): State<AppState>, _auth: AuthUser) -> ApiResult<Vec<PlanView>> {
    Ok(axum::Json(s.service.plans().await?))
}

pub async fn overview(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
) -> ApiResult<BillingOverview> {
    Ok(axum::Json(s.service.overview(ws, auth.user_id()).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBody {
    pub plan_id: Option<Uuid>,
    pub top_up_credits: Option<i64>,
}

pub async fn checkout(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
    Json(body): Json<CheckoutBody>,
) -> ApiResult<CheckoutUrl> {
    let target = match (body.plan_id, body.top_up_credits) {
        (Some(p), None) => CheckoutTarget::Plan(p),
        (None, Some(c)) => CheckoutTarget::TopUp(c),
        _ => {
            return Err(AppError::validation(
                "INVALID_CHECKOUT",
                "send exactly one of planId or topUpCredits",
            ));
        }
    };
    Ok(axum::Json(s.service.checkout(ws, auth.user_id(), target).await?))
}

pub async fn portal(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
) -> ApiResult<CheckoutUrl> {
    Ok(axum::Json(s.service.portal(ws, auth.user_id()).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_by: String,
}

pub async fn usage(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
    Query(q): Query<UsageQuery>,
) -> ApiResult<UsageReport> {
    Ok(axum::Json(
        s.service
            .usage(ws, auth.user_id(), q.from, q.to, &q.group_by)
            .await?,
    ))
}

pub async fn transactions(
    State(s): State<AppState>,
    auth: AuthUser,
    Path(ws): Path<WorkspaceId>,
    Query(page): Query<PageRequest>,
) -> ApiResult<Page<TransactionView>> {
    Ok(axum::Json(
        s.service.transactions(ws, auth.user_id(), &page).await?,
    ))
}

/// The raw body is what Stripe signed: extract bytes, never re-serialise.
pub async fn stripe_webhook(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<WebhookAck> {
    let sig = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok());
    Ok(axum::Json(s.service.handle_webhook(&body, sig).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateCardBody {
    pub entries: Vec<RateCardEntryBody>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateCardEntryBody {
    pub charge_type: String,
    pub unit: String,
    pub model: Option<String>,
    pub unit_price: f64,
    pub notes: Option<String>,
}

pub async fn set_rate_card(
    State(s): State<AppState>,
    admin: SystemAdmin,
    Json(body): Json<RateCardBody>,
) -> ApiResult<Value> {
    let n = s
        .service
        .admin_set_rate_card(
            admin.0.sub,
            body.entries
                .into_iter()
                .map(|e| RateCardEdit {
                    charge_type: e.charge_type,
                    unit: e.unit,
                    model: e.model,
                    unit_price: e.unit_price,
                    notes: e.notes,
                })
                .collect(),
        )
        .await?;
    Ok(axum::Json(json!({ "updated": n })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustBody {
    pub amount: i64,
    pub reason: String,
    pub idempotency_key: Option<String>,
}

pub async fn adjust_credits(
    State(s): State<AppState>,
    admin: SystemAdmin,
    Path(ws): Path<WorkspaceId>,
    Json(body): Json<AdjustBody>,
) -> ApiResult<AdjustResult> {
    Ok(axum::Json(
        s.service
            .admin_adjust(
                admin.0.sub,
                ws,
                body.amount,
                &body.reason,
                body.idempotency_key.as_deref(),
            )
            .await?,
    ))
}
