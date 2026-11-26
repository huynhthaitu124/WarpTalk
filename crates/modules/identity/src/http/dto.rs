use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use warptalk_kernel::UserId;

use crate::application::{AuthSession, MeView};
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
pub struct EmailRequest {
    pub email: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleTokenRequest {
    pub id_token: String,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeDto {
    pub user_id: UserId,
    pub email: String,
    pub full_name: String,
    pub avatar_url: Option<String>,
    pub preferred_language: String,
    pub timezone: String,
    pub email_verified: bool,
    pub system_admin: bool,
    pub has_password: bool,
    pub google_linked: bool,
}

impl From<MeView> for MeDto {
    fn from(m: MeView) -> Self {
        Self {
            user_id: m.user_id,
            email: m.email,
            full_name: m.profile.full_name,
            avatar_url: m.profile.avatar_url,
            preferred_language: m.profile.preferred_language,
            timezone: m.profile.timezone,
            email_verified: m.email_verified,
            system_admin: m.system_admin,
            has_password: m.has_password,
            google_linked: m.google_linked,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantConsentRequest {
    pub granted: bool,
    pub consent_version: String,
    /// SHA-256 hex of the consent text the user acknowledged.
    pub acknowledgement_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollRequest {
    pub sample_base64: String,
    /// `wav` (default) or `pcm16` (raw mono little-endian; needs `sampleRate`).
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentDto {
    pub granted: bool,
    pub consent_version: Option<String>,
    pub granted_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProfileDto {
    pub id: uuid::Uuid,
    pub status: String,
    pub language: Option<String>,
    pub quality_score: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatusDto {
    pub consent: ConsentDto,
    pub profile: Option<VoiceProfileDto>,
    /// Consent active and a provider voice exists: dubbing may use the user's cloned voice.
    pub clone_enabled: bool,
}

impl From<crate::application::voice::VoiceStatus> for VoiceStatusDto {
    fn from(s: crate::application::voice::VoiceStatus) -> Self {
        let clone_enabled = s.clone_enabled();
        Self {
            consent: ConsentDto {
                granted: s.consent.is_some(),
                consent_version: s.consent.as_ref().map(|c| c.version.clone()),
                granted_at: s.consent.as_ref().map(|c| c.granted_at),
            },
            profile: s.profile.map(|p| VoiceProfileDto {
                id: p.id,
                status: p.status,
                language: p.language,
                quality_score: p.quality_score,
                created_at: p.created_at,
            }),
            clone_enabled,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub profile_id: uuid::Uuid,
    pub status: &'static str,
    pub quality_score: i32,
    pub duration_seconds: f32,
    pub language: String,
}

/// One signed-in session. `id` is the session (refresh-token family) id.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSessionDto {
    pub id: uuid::Uuid,
    pub device_info: Option<String>,
    pub ip_address: Option<String>,
    pub signed_in_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub is_current: bool,
}

impl UserSessionDto {
    pub fn new(s: crate::application::ports::SessionRecord, current: uuid::Uuid) -> Self {
        Self {
            id: s.family_id,
            is_current: s.family_id == current,
            device_info: s.device_info,
            ip_address: s.ip_address,
            signed_in_at: s.signed_in_at,
            last_active_at: s.last_active_at,
            expires_at: s.expires_at,
        }
    }
}
