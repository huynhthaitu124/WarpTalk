use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::ports::ChatMessage;

use crate::application::service::JoinGrant;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinResponse {
    pub room_name: String,
    pub server_url: String,
    pub token: String,
    pub participant_identity: String,
}

impl From<JoinGrant> for JoinResponse {
    fn from(g: JoinGrant) -> Self {
        Self {
            room_name: g.room_name,
            server_url: g.server_url,
            token: g.token,
            participant_identity: g.participant_identity,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostChatRequest {
    pub content: String,
    pub client_message_id: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct HideChatRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryQuery {
    pub before: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageDto {
    pub message_id: Uuid,
    pub sender_id: Option<Uuid>,
    pub sender_name: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub hidden: bool,
    pub translations: BTreeMap<String, String>,
}

impl From<ChatMessage> for ChatMessageDto {
    fn from(m: ChatMessage) -> Self {
        Self {
            message_id: m.id,
            sender_id: m.sender_id.map(|u| u.as_uuid()),
            sender_name: m.sender_name,
            content: m.content,
            created_at: m.created_at,
            hidden: m.hidden,
            translations: m.translations,
        }
    }
}
