//! Publishes workspace AI config changes on `warptalk:ai-config:changed`.

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use warptalk_assistant::application::ports::ConfigNotifier;
use warptalk_assistant::domain::byo::{AI_CONFIG_CHANNEL, ConfigChanged};
use warptalk_kernel::WorkspaceId;

pub struct RedisConfigNotifier {
    conn: ConnectionManager,
}

impl RedisConfigNotifier {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl ConfigNotifier for RedisConfigNotifier {
    async fn changed(&self, workspace: WorkspaceId) {
        let mut conn = self.conn.clone();
        let ev = ConfigChanged {
            workspace_id: workspace.as_uuid(),
        };
        if let Err(e) = warptalk_bus::publish_json(&mut conn, AI_CONFIG_CHANNEL, &ev).await {
            // Replicas still pick the change up when their cache entry expires.
            tracing::warn!(error = %e, "failed to publish ai-config change");
        }
    }
}
