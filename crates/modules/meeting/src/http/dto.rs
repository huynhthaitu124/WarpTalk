use serde::{Deserialize, Serialize};

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
