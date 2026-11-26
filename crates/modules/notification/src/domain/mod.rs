//! Entities, value objects, state machines. Pure Rust, no IO.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The single preference row per user the product keeps (spec 05a allows one per type).
pub const DEFAULT_NOTIFICATION_TYPE: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreference {
    pub id: Uuid,
    pub user_id: Uuid,
    pub notification_type: String,
    pub email_enabled: bool,
    pub push_enabled: bool,
    pub in_app_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

/// Partial update: `None` keeps the stored value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreferenceUpdate {
    pub email_enabled: Option<bool>,
    pub push_enabled: Option<bool>,
    pub in_app_enabled: Option<bool>,
}

impl PreferenceUpdate {
    pub fn is_empty(&self) -> bool {
        self.email_enabled.is_none() && self.push_enabled.is_none() && self.in_app_enabled.is_none()
    }
}
