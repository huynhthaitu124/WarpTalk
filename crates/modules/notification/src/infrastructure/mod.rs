//! Adapters: repositories (sqlx), Redis publishers/consumers, provider clients.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::AppResult;

use crate::application::PreferenceRepository;
use crate::domain::{NotificationPreference, PreferenceUpdate};

pub struct PgPreferenceRepository {
    pool: PgPool,
}

impl PgPreferenceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLS: &str =
    "id, user_id, notification_type, email_enabled, push_enabled, in_app_enabled, updated_at";

fn preference(r: &sqlx::postgres::PgRow) -> Result<NotificationPreference, sqlx::Error> {
    Ok(NotificationPreference {
        id: r.try_get("id")?,
        user_id: r.try_get("user_id")?,
        notification_type: r.try_get("notification_type")?,
        email_enabled: r.try_get("email_enabled")?,
        push_enabled: r.try_get("push_enabled")?,
        in_app_enabled: r.try_get("in_app_enabled")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl PreferenceRepository for PgPreferenceRepository {
    async fn get_or_create(
        &self,
        user: Uuid,
        notification_type: &str,
        new_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<NotificationPreference> {
        sqlx::query(
            "INSERT INTO notification.notification_preferences
                (id, user_id, notification_type, email_enabled, push_enabled, in_app_enabled,
                 updated_at)
             VALUES ($1, $2, $3, true, true, true, $4)
             ON CONFLICT (user_id, notification_type) DO NOTHING",
        )
        .bind(new_id)
        .bind(user)
        .bind(notification_type)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let row = sqlx::query(&format!(
            "SELECT {COLS} FROM notification.notification_preferences
              WHERE user_id = $1 AND notification_type = $2"
        ))
        .bind(user)
        .bind(notification_type)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        preference(&row).map_err(map_sqlx_error)
    }

    async fn upsert(
        &self,
        user: Uuid,
        notification_type: &str,
        update: PreferenceUpdate,
        new_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<NotificationPreference> {
        let row = sqlx::query(&format!(
            "INSERT INTO notification.notification_preferences AS p
                (id, user_id, notification_type, email_enabled, push_enabled, in_app_enabled,
                 updated_at)
             VALUES ($1, $2, $3, COALESCE($4, true), COALESCE($5, true), COALESCE($6, true), $7)
             ON CONFLICT (user_id, notification_type) DO UPDATE
                SET email_enabled  = COALESCE($4, p.email_enabled),
                    push_enabled   = COALESCE($5, p.push_enabled),
                    in_app_enabled = COALESCE($6, p.in_app_enabled),
                    updated_at     = $7
             RETURNING {COLS}"
        ))
        .bind(new_id)
        .bind(user)
        .bind(notification_type)
        .bind(update.email_enabled)
        .bind(update.push_enabled)
        .bind(update.in_app_enabled)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        preference(&row).map_err(map_sqlx_error)
    }
}
