use uuid::Uuid;
use warptalk_kernel::UserId;

/// Minimal user view exposed to other modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub id: UserId,
    pub email: String,
    pub full_name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub full_name: String,
    pub avatar_url: Option<String>,
    pub preferred_language: String,
    pub timezone: String,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub default_audio_input: Option<String>,
    pub default_audio_output: Option<String>,
    pub auto_translate_enabled: bool,
    pub default_speak_language: String,
    pub default_listen_language: String,
}

/// Opaque family id helper type alias for readability.
pub type FamilyId = Uuid;
