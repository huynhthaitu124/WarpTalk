//! Writes the AI service state keys other services read (spec 10 §5.4).

use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use warptalk_contracts::keys::{workspace_ai_service_state, workspace_ai_service_suspended};
use warptalk_kernel::WorkspaceId;

use crate::application::ports::ServiceStateSink;
use crate::domain::ServiceState;

pub struct RedisStateSink {
    conn: ConnectionManager,
}

impl RedisStateSink {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl ServiceStateSink for RedisStateSink {
    async fn publish(&self, ws: WorkspaceId, state: &ServiceState) {
        let mut c = self.conn.clone();
        let r: redis::RedisResult<()> = async {
            let _: () = c.set(workspace_ai_service_state(ws), state.as_str()).await?;
            match state.suspended_reason() {
                Some(reason) => {
                    let _: () = c.set(workspace_ai_service_suspended(ws), reason).await?;
                }
                None => {
                    let _: i64 = c.del(workspace_ai_service_suspended(ws)).await?;
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::warn!(workspace_id = %ws, error = %e, "could not publish AI service state");
        }
    }
}
