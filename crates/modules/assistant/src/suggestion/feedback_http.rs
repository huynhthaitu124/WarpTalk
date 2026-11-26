//! Suggestion feedback loop REST (paths relative to `/api/v1`).
//!
//! - `POST /suggestions/{id}/feedback` `{ "action": "used|dismissed|helpful|not_helpful|wrong",
//!   "comment"?: "..." }`: any user who can view the suggestion's room (host, participant,
//!   workspace OWNER/ADMIN). One verdict per user and action; repeating it updates the comment.
//! - `GET /workspaces/{id}/suggestions/quality?from&to`: workspace OWNER/ADMIN (or system admin);
//!   shown / used / dismissed / wrong counts and rates per category. `from`/`to` are RFC 3339 or
//!   `YYYY-MM-DD` (default: the last 30 days).
//! - `GET /admin/suggestions/export?from&to&label=wrong|not_helpful&limit`: system admin;
//!   anonymised bad cases as `{ "meetings": [ ... ] }`, each element in the `warptalk-ai-eval`
//!   dataset shape (`apps/ai-eval/src/dataset.rs`), with the bad output as a forbidden
//!   expectation, so a file can be dropped into `datasets/`.
//!
//! Proposal (not implemented): per-workspace auto-tuning of the category confidence floor. A
//! nightly job reads [`super::store::PgSuggestionStore::quality`] per workspace and category and,
//! with at least N shown (say 50) and a wrong rate above a target (say 15 %), raises
//! `category_rules.<cat>.min_confidence` by a small step (0.02, capped at +0.15 above the
//! default); a dismissed + wrong rate below 5 % lowers it by the same step (floored at the
//! default). Changes go through the settings store (workspace scope, audited), never silently.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, RoomId, WorkspaceId, WorkspaceRole};
use warptalk_web::extract::{Json, Path, Query};
use warptalk_web::{AuthUser, JwtKeys, SystemAdmin};

use super::config::HygieneConfig;
use super::hygiene;
use super::store::{ExportCase, FeedbackAction, PgSuggestionStore, QualityReport};
use crate::application::ports::WorkspaceAccess;
use crate::summary::RoomAccess;

const COMMENT_MAX_CHARS: usize = 1000;
const DEFAULT_RANGE_DAYS: i64 = 30;
const MAX_RANGE_DAYS: i64 = 366;
const DEFAULT_EXPORT_LIMIT: i64 = 200;
const MAX_EXPORT_LIMIT: i64 = 1000;

/// Everything the feedback routes need.
#[derive(Clone)]
pub struct FeedbackApi {
    pub store: Arc<PgSuggestionStore>,
    pub access: Arc<dyn RoomAccess>,
    pub members: Arc<dyn WorkspaceAccess>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
struct State_ {
    api: FeedbackApi,
    jwt: JwtKeys,
}

impl FromRef<State_> for JwtKeys {
    fn from_ref(s: &State_) -> JwtKeys {
        s.jwt.clone()
    }
}

pub fn router(api: FeedbackApi, jwt: JwtKeys) -> Router {
    Router::new()
        .route("/suggestions/{id}/feedback", post(post_feedback))
        .route("/workspaces/{id}/suggestions/quality", get(quality))
        .route("/admin/suggestions/export", get(export))
        .with_state(State_ { api, jwt })
}

#[derive(Deserialize)]
struct FeedbackBody {
    action: FeedbackAction,
    #[serde(default)]
    comment: Option<String>,
}

async fn post_feedback(
    State(s): State<State_>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<FeedbackBody>,
) -> AppResult<StatusCode> {
    let meta = s
        .api
        .store
        .meta(id)
        .await?
        .ok_or_else(|| AppError::not_found("suggestion not found"))?;
    let info = s
        .api
        .access
        .room_access(RoomId::from_uuid(meta.scope.room_id), auth.user_id())
        .await?
        .ok_or_else(|| AppError::not_found("suggestion not found"))?;
    // The room must belong to the suggestion's workspace and the caller must be able to view it.
    if info.workspace_id.as_uuid() != meta.scope.workspace_id || !info.can_view() {
        return Err(AppError::forbidden("not allowed to rate this suggestion"));
    }
    let comment = body
        .comment
        .map(|c| hygiene::truncate_chars(c.trim(), COMMENT_MAX_CHARS))
        .map(|c| {
            c.chars()
                .filter(|ch| !ch.is_control() || *ch == '\n')
                .collect::<String>()
        })
        .filter(|c| !c.is_empty());
    s.api
        .store
        .upsert_feedback(
            id,
            meta.scope,
            auth.user_id().as_uuid(),
            body.action,
            comment.as_deref(),
        )
        .await?;
    metrics::counter!(
        "warptalk_suggestion_feedback_total",
        "category" => meta.category,
        "model" => meta.model,
        "action" => body.action.as_str()
    )
    .increment(1);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
}

/// RFC 3339 or `YYYY-MM-DD` (start of that day, UTC).
fn parse_time(field: &str, v: &str) -> AppResult<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(v) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d")
        && let Some(t) = d.and_hms_opt(0, 0, 0)
    {
        return Ok(t.and_utc());
    }
    Err(AppError::validation(
        "INVALID_RANGE",
        format!("{field} must be RFC 3339 or YYYY-MM-DD"),
    ))
}

fn range(q: &RangeQuery, now: DateTime<Utc>) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let to = match q.to.as_deref() {
        Some(v) => parse_time("to", v)?,
        None => now,
    };
    let from = match q.from.as_deref() {
        Some(v) => parse_time("from", v)?,
        None => to - Duration::days(DEFAULT_RANGE_DAYS),
    };
    if from >= to {
        return Err(AppError::validation(
            "INVALID_RANGE",
            "from must be before to",
        ));
    }
    if to - from > Duration::days(MAX_RANGE_DAYS) {
        return Err(AppError::validation(
            "INVALID_RANGE",
            "range is limited to 366 days",
        ));
    }
    Ok((from, to))
}

async fn quality(
    State(s): State<State_>,
    auth: AuthUser,
    Path(ws): Path<Uuid>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<QualityReport>> {
    let (from, to) = range(&q, s.api.clock.now())?;
    if !auth.0.sa {
        let role = s
            .api
            .members
            .member_role(WorkspaceId::from_uuid(ws), auth.user_id())
            .await?;
        if !matches!(role, Some(WorkspaceRole::Owner | WorkspaceRole::Admin)) {
            return Err(AppError::forbidden(
                "workspace owner or admin role required",
            ));
        }
    }
    Ok(Json(s.api.store.quality(ws, from, to).await?))
}

#[derive(Deserialize)]
struct ExportQuery {
    from: Option<String>,
    to: Option<String>,
    label: Option<String>,
    limit: Option<i64>,
}

async fn export(
    State(s): State<State_>,
    _admin: SystemAdmin,
    Query(q): Query<ExportQuery>,
) -> AppResult<Json<Value>> {
    let (from, to) = range(
        &RangeQuery {
            from: q.from.clone(),
            to: q.to.clone(),
        },
        s.api.clock.now(),
    )?;
    let label = match q.label.as_deref().unwrap_or("wrong") {
        "wrong" => FeedbackAction::Wrong,
        "not_helpful" => FeedbackAction::NotHelpful,
        _ => {
            return Err(AppError::validation(
                "INVALID_LABEL",
                "label must be wrong or not_helpful",
            ));
        }
    };
    let limit = q
        .limit
        .unwrap_or(DEFAULT_EXPORT_LIMIT)
        .clamp(1, MAX_EXPORT_LIMIT);
    let cases = s.api.store.export(from, to, label, limit).await?;
    let meetings: Vec<Value> = cases.iter().filter_map(case_to_meeting).collect();
    Ok(Json(
        json!({ "count": meetings.len(), "meetings": meetings }),
    ))
}

/// One bad case as an ai-eval `Meeting`: speakers renamed `Speaker N`, PII masked, ids dropped.
/// `None` when the case has no transcript lines (the dataset requires at least one).
pub fn case_to_meeting(c: &ExportCase) -> Option<Value> {
    if c.context.is_empty() {
        return None;
    }
    let pii = HygieneConfig::default();
    let mut names: HashMap<&str, String> = HashMap::new();
    let mut participants: Vec<String> = Vec::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut prev_t = 0_i64;
    let mut lines: Vec<Value> = Vec::new();
    for l in &c.context {
        let next_name = format!("Speaker {}", names.len() + 1);
        let speaker = names.entry(l.speaker.as_str()).or_insert(next_name).clone();
        if !participants.contains(&speaker) {
            participants.push(speaker.clone());
        }
        let mut text = hygiene::scrub_pii(&l.text, &pii);
        // The dataset needs unique line texts.
        let n = seen.entry(text.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            text = format!("{text} ({n})");
        }
        let channel = match l.channel.as_str() {
            "me" | "others" => l.channel.as_str(),
            _ => "unknown",
        };
        prev_t = prev_t.max(l.t_ms.max(0));
        lines.push(json!({ "t_ms": prev_t, "channel": channel, "speaker": speaker, "text": text }));
    }
    let last = lines.len() - 1;
    let bad = hygiene::scrub_pii(&c.content, &pii);
    let must_not: Vec<String> = if c.label == "wrong" && !bad.is_empty() {
        vec![bad.clone()]
    } else {
        Vec::new()
    };
    Some(json!({
        "id": format!("fb-{}", c.id),
        "language": if c.language.trim().is_empty() { "en" } else { c.language.as_str() },
        "title": format!("User feedback ({}): {}", c.label, c.category),
        "participants": participants,
        "lines": lines,
        "snippets": [],
        "expectations": {
            "suggestions": {
                "expected": [],
                "forbidden": [{ "at_line": last, "category": c.category }],
                "must_not_include": must_not
            },
            "summary": { "decisions": [], "action_items": [], "must_not_include": [] }
        },
        "fake": { "suggestions": [] },
        "feedback": {
            "label": c.label,
            "count": c.feedback_count,
            "category": c.category,
            "output": bad,
            "detail": hygiene::scrub_pii(&c.detail, &pii),
            "model": c.model,
            "prompt_version": c.prompt_version
        }
    }))
}
