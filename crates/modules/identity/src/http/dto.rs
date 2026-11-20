use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use warptalk_kernel::UserId;

use crate::application::AuthSession;
use crate::domain::{Profile, Settings};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub full_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub user_id: UserId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub user_id: UserId,
    pub email: String,
    pub access_token: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub refresh_token: String,
}

impl From<AuthSession> for AuthResponse {
    fn from(s: AuthSession) -> Self {
        Self {
            user_id: s.user_id,
            email: s.email,
            access_token: s.access_token,
            access_token_expires_at: s.access_token_expires_at,
            refresh_token: s.refresh_token,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub full_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub preferred_language: String,
    /// Optional on PUT (kept when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl From<Profile> for ProfileDto {
    fn from(p: Profile) -> Self {
        Self {
            full_name: p.full_name,
            avatar_url: p.avatar_url,
            preferred_language: p.preferred_language,
            timezone: Some(p.timezone),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    #[serde(default)]
    pub default_audio_input: Option<String>,
    #[serde(default)]
    pub default_audio_output: Option<String>,
    pub auto_translate_enabled: bool,
    /// Optional on PUT (kept when absent).
    #[serde(default)]
    pub default_speak_language: Option<String>,
    #[serde(default)]
    pub default_listen_language: Option<String>,
}

impl From<Settings> for SettingsDto {
    fn from(s: Settings) -> Self {
        Self {
            default_audio_input: s.default_audio_input,
            default_audio_output: s.default_audio_output,
            auto_translate_enabled: s.auto_translate_enabled,
            default_speak_language: Some(s.default_speak_language),
            default_listen_language: Some(s.default_listen_language),
        }
    }
}
