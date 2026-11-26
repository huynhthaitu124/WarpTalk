//! Entitlement snapshot: what billing resolved for a workspace (plan, overrides, limits).
//! Values travel as wire strings ("true", "20") exactly as billing published them.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntitlementKind {
    Flag,
    Limit,
    Text,
}

/// One resolved entitlement as stored in the snapshot's `entitlements` object (keyed by key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementValue {
    pub kind: EntitlementKind,
    pub value: String,
    /// `platform_default`, `plan:<slug>`, `contract_override` or `workspace_override`.
    pub source: String,
    #[serde(default)]
    pub ceiling: Option<String>,
    #[serde(default)]
    pub ceiling_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitlementSnapshot {
    pub plan_slug: Option<String>,
    pub has_active_subscription: bool,
    pub resolved_at: DateTime<Utc>,
    /// Sorted by key.
    pub entitlements: BTreeMap<String, EntitlementValue>,
}
