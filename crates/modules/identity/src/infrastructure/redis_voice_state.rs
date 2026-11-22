use redis::aio::ConnectionManager;
use warptalk_kernel::{AppError, AppResult, UserId};

use crate::application::voice::{VoiceRuntimeState, VoiceStatePublisher};

/// Key the pipeline reads (no database there): `voice:consent:{userId}` holds
/// `{"consent": bool, "voiceId": string?}`. A missing key means no consent.
pub fn consent_key(user: UserId) -> String {
    format!("voice:consent:{}", user.as_uuid())
}

pub struct RedisVoiceStatePublisher {
    conn: ConnectionManager,
}

impl RedisVoiceStatePublisher {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl VoiceStatePublisher for RedisVoiceStatePublisher {
    async fn publish(&self, user: UserId, state: &VoiceRuntimeState) -> AppResult<()> {
        let mut v = serde_json::json!({ "consent": state.consent });
        if let Some(id) = &state.provider_voice_id {
            v["voiceId"] = serde_json::Value::String(id.clone());
        }
        let mut conn = self.conn.clone();
        redis::cmd("SET")
            .arg(consent_key(user))
            .arg(v.to_string())
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "publishing voice consent state failed");
                AppError::unavailable("voice consent state could not be published")
            })
    }
}
