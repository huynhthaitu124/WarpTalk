//! Durable snapshot of published suggestions and the feedback they receive (schema `assistant`).
//!
//! The engine records every published suggestion through [`SuggestionStore`] (best effort, never
//! blocks or fails delivery). [`PgSuggestionStore`] also implements the feedback write, the
//! per-workspace quality aggregate and the anonymised export of bad cases.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult};

/// One transcript line the model saw (already PII-scrubbed as sent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextLine {
    /// Milliseconds since the first line of the window.
    pub t_ms: i64,
    /// `me | others | unknown`.
    pub channel: String,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SuggestionRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub room_id: Uuid,
    pub segment_id: String,
    pub category: String,
    pub severity: String,
    pub content: String,
    pub detail: String,
    pub language: String,
    pub model: String,
    pub prompt_version: String,
    pub confidence: f32,
    pub sources: Value,
    pub context: Vec<ContextLine>,
}

#[async_trait]
pub trait SuggestionStore: Send + Sync {
    /// Idempotent on `id`.
    async fn record(&self, r: &SuggestionRecord) -> AppResult<()>;
}

/// What a user did with a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackAction {
    Used,
    Dismissed,
    Helpful,
    NotHelpful,
    Wrong,
}

impl FeedbackAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Dismissed => "dismissed",
            Self::Helpful => "helpful",
            Self::NotHelpful => "not_helpful",
            Self::Wrong => "wrong",
        }
    }
}

/// Where a suggestion lives (for access checks).
#[derive(Debug, Clone, Copy)]
pub struct SuggestionScope {
    pub workspace_id: Uuid,
    pub room_id: Uuid,
}

/// Suggestion facts the feedback metrics are labelled with.
#[derive(Debug, Clone)]
pub struct SuggestionMeta {
    pub scope: SuggestionScope,
    pub category: String,
    pub model: String,
}

/// Quality counters of one category.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryQuality {
    pub category: String,
    pub shown: i64,
    pub used: i64,
    pub dismissed: i64,
    pub helpful: i64,
    pub not_helpful: i64,
    pub wrong: i64,
    /// `wrong / shown` (0 when nothing was shown).
    pub wrong_rate: f64,
    /// `used / shown`.
    pub used_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub total: CategoryQuality,
    pub categories: Vec<CategoryQuality>,
}

/// A bad case ready to become eval data.
#[derive(Debug, Clone)]
pub struct ExportCase {
    pub id: Uuid,
    pub category: String,
    pub language: String,
    pub content: String,
    pub detail: String,
    pub model: String,
    pub prompt_version: String,
    pub label: String,
    pub feedback_count: i64,
    pub context: Vec<ContextLine>,
}

fn rate(n: i64, d: i64) -> f64 {
    if d <= 0 { 0.0 } else { n as f64 / d as f64 }
}

fn finish_quality(mut q: CategoryQuality) -> CategoryQuality {
    q.wrong_rate = rate(q.wrong, q.shown);
    q.used_rate = rate(q.used, q.shown);
    q
}

pub struct PgSuggestionStore {
    pool: PgPool,
}

impl PgSuggestionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn meta(&self, id: Uuid) -> AppResult<Option<SuggestionMeta>> {
        let row: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
            "SELECT workspace_id, room_id, category, model FROM assistant.suggestions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(
            row.map(|(workspace_id, room_id, category, model)| SuggestionMeta {
                scope: SuggestionScope {
                    workspace_id,
                    room_id,
                },
                category,
                model,
            }),
        )
    }

    /// Insert or update the user's verdict (one row per suggestion, user and action).
    pub async fn upsert_feedback(
        &self,
        suggestion: Uuid,
        scope: SuggestionScope,
        user: Uuid,
        action: FeedbackAction,
        comment: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO assistant.suggestion_feedback \
                 (id, suggestion_id, workspace_id, room_id, user_id, action, comment) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (suggestion_id, user_id, action) \
             DO UPDATE SET comment = EXCLUDED.comment, created_at = now()",
        )
        .bind(Uuid::now_v7())
        .bind(suggestion)
        .bind(scope.workspace_id)
        .bind(scope.room_id)
        .bind(user)
        .bind(action.as_str())
        .bind(comment)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    /// Shown / used / dismissed / wrong per category over `[from, to)` of the suggestions'
    /// creation time. Always filtered by `workspace_id`.
    pub async fn quality(
        &self,
        workspace: Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AppResult<QualityReport> {
        let rows: Vec<(String, i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT s.category, count(DISTINCT s.id), \
                    count(DISTINCT s.id) FILTER (WHERE f.action = 'used'), \
                    count(DISTINCT s.id) FILTER (WHERE f.action = 'dismissed'), \
                    count(DISTINCT s.id) FILTER (WHERE f.action = 'helpful'), \
                    count(DISTINCT s.id) FILTER (WHERE f.action = 'not_helpful'), \
                    count(DISTINCT s.id) FILTER (WHERE f.action = 'wrong') \
             FROM assistant.suggestions s \
             LEFT JOIN assistant.suggestion_feedback f \
                    ON f.suggestion_id = s.id AND f.workspace_id = s.workspace_id \
             WHERE s.workspace_id = $1 AND s.created_at >= $2 AND s.created_at < $3 \
             GROUP BY s.category ORDER BY s.category",
        )
        .bind(workspace)
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let mut total = CategoryQuality {
            category: "*".into(),
            shown: 0,
            used: 0,
            dismissed: 0,
            helpful: 0,
            not_helpful: 0,
            wrong: 0,
            wrong_rate: 0.0,
            used_rate: 0.0,
        };
        let mut categories = Vec::new();
        for (category, shown, used, dismissed, helpful, not_helpful, wrong) in rows {
            total.shown += shown;
            total.used += used;
            total.dismissed += dismissed;
            total.helpful += helpful;
            total.not_helpful += not_helpful;
            total.wrong += wrong;
            categories.push(finish_quality(CategoryQuality {
                category,
                shown,
                used,
                dismissed,
                helpful,
                not_helpful,
                wrong,
                wrong_rate: 0.0,
                used_rate: 0.0,
            }));
        }
        Ok(QualityReport {
            from,
            to,
            total: finish_quality(total),
            categories,
        })
    }

    /// Suggestions labelled `label` (`wrong` or `not_helpful`) by at least one user whose
    /// feedback fell in `[from, to)`. Cross-workspace: system admins only.
    pub async fn export(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        label: FeedbackAction,
        limit: i64,
    ) -> AppResult<Vec<ExportCase>> {
        type Row = (
            Uuid,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            Value,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT s.id, s.category, s.language, s.content, s.detail, s.model, s.prompt_version, \
                    count(f.id), s.context \
             FROM assistant.suggestions s \
             JOIN assistant.suggestion_feedback f ON f.suggestion_id = s.id \
             WHERE f.action = $3 AND f.created_at >= $1 AND f.created_at < $2 \
             GROUP BY s.id ORDER BY max(f.created_at) DESC LIMIT $4",
        )
        .bind(from)
        .bind(to)
        .bind(label.as_str())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, category, language, content, detail, model, prompt_version, n, ctx)| {
                    ExportCase {
                        id,
                        category,
                        language,
                        content,
                        detail,
                        model,
                        prompt_version,
                        label: label.as_str().to_string(),
                        feedback_count: n,
                        context: serde_json::from_value(ctx).unwrap_or_default(),
                    }
                },
            )
            .collect())
    }
}

#[async_trait]
impl SuggestionStore for PgSuggestionStore {
    async fn record(&self, r: &SuggestionRecord) -> AppResult<()> {
        let context = serde_json::to_value(&r.context)
            .map_err(|e| AppError::internal(format!("suggestion context: {e}")))?;
        sqlx::query(
            "INSERT INTO assistant.suggestions \
                 (id, workspace_id, room_id, segment_id, category, severity, content, detail, \
                  language, model, prompt_version, confidence, sources, context) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(r.id)
        .bind(r.workspace_id)
        .bind(r.room_id)
        .bind(&r.segment_id)
        .bind(&r.category)
        .bind(&r.severity)
        .bind(&r.content)
        .bind(&r.detail)
        .bind(&r.language)
        .bind(&r.model)
        .bind(&r.prompt_version)
        .bind(r.confidence)
        .bind(&r.sources)
        .bind(context)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }
}
