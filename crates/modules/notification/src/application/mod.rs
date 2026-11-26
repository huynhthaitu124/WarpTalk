//! Use cases (commands/queries) and ports (traits) implemented elsewhere.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppResult, Clock};

use crate::domain::{DEFAULT_NOTIFICATION_TYPE, NotificationPreference, PreferenceUpdate};

#[async_trait]
pub trait PreferenceRepository: Send + Sync {
    /// The user's row for `notification_type`, created with every channel on when missing.
    async fn get_or_create(
        &self,
        user: Uuid,
        notification_type: &str,
        new_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<NotificationPreference>;

    /// Upsert: omitted fields keep their stored value (new rows start with every channel on).
    async fn upsert(
        &self,
        user: Uuid,
        notification_type: &str,
        update: PreferenceUpdate,
        new_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<NotificationPreference>;
}

pub struct PreferenceService {
    repo: Arc<dyn PreferenceRepository>,
    clock: Arc<dyn Clock>,
}

impl PreferenceService {
    pub fn new(repo: Arc<dyn PreferenceRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, clock }
    }

    pub async fn get(&self, user: Uuid) -> AppResult<NotificationPreference> {
        self.repo
            .get_or_create(
                user,
                DEFAULT_NOTIFICATION_TYPE,
                Uuid::now_v7(),
                self.clock.now(),
            )
            .await
    }

    pub async fn update(
        &self,
        user: Uuid,
        update: PreferenceUpdate,
    ) -> AppResult<NotificationPreference> {
        if update.is_empty() {
            return self.get(user).await;
        }
        self.repo
            .upsert(
                user,
                DEFAULT_NOTIFICATION_TYPE,
                update,
                Uuid::now_v7(),
                self.clock.now(),
            )
            .await
    }
}
