//! Profile and settings use cases (BR-07: callers only touch their own data).

use std::sync::Arc;

use warptalk_kernel::{AppError, AppResult, Clock, UserId};

use super::ports::IdentityRepository;
use crate::domain::{Profile, Settings, validation};

pub struct ProfileService {
    repo: Arc<dyn IdentityRepository>,
    clock: Arc<dyn Clock>,
}

fn invalid(m: &str) -> AppError {
    AppError::validation("VALIDATION_ERROR", m)
}

impl ProfileService {
    pub fn new(repo: Arc<dyn IdentityRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, clock }
    }

    pub async fn get_profile(&self, id: UserId) -> AppResult<Profile> {
        self.repo
            .get_profile(id)
            .await?
            .ok_or_else(|| AppError::not_found("user not found"))
    }

    /// `timezone: None` keeps the current value.
    pub async fn update_profile(
        &self,
        id: UserId,
        full_name: &str,
        avatar_url: Option<&str>,
        preferred_language: &str,
        timezone: Option<&str>,
    ) -> AppResult<Profile> {
        let current = self.get_profile(id).await?;
        let profile = Profile {
            full_name: validation::full_name(full_name).map_err(invalid)?,
            avatar_url: match avatar_url.map(str::trim).filter(|s| !s.is_empty()) {
                Some(u) => Some(validation::avatar_url(u).map_err(invalid)?),
                None => None,
            },
            preferred_language: validation::language_tag(preferred_language).map_err(invalid)?,
            timezone: match timezone {
                Some(t) => validation::timezone(t).map_err(invalid)?,
                None => current.timezone,
            },
        };
        self.repo
            .update_profile(id, &profile, self.clock.now())
            .await?;
        Ok(profile)
    }

    pub async fn get_settings(&self, id: UserId) -> AppResult<Settings> {
        self.repo.get_settings(id, self.clock.now()).await
    }

    /// Language fields: `None` keeps the current value. Audio devices: `None` clears.
    pub async fn update_settings(&self, id: UserId, input: SettingsUpdate) -> AppResult<Settings> {
        let current = self.get_settings(id).await?;
        let device = |v: Option<String>| -> AppResult<Option<String>> {
            match v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) {
                Some(d) => Ok(Some(validation::device_name(&d).map_err(invalid)?)),
                None => Ok(None),
            }
        };
        let settings = Settings {
            default_audio_input: device(input.default_audio_input)?,
            default_audio_output: device(input.default_audio_output)?,
            auto_translate_enabled: input.auto_translate_enabled,
            default_speak_language: match input.default_speak_language {
                Some(l) => validation::language_tag(&l).map_err(invalid)?,
                None => current.default_speak_language,
            },
            default_listen_language: match input.default_listen_language {
                Some(l) => validation::language_tag(&l).map_err(invalid)?,
                None => current.default_listen_language,
            },
        };
        self.repo
            .update_settings(id, &settings, self.clock.now())
            .await?;
        Ok(settings)
    }
}

#[derive(Debug, Clone)]
pub struct SettingsUpdate {
    pub default_audio_input: Option<String>,
    pub default_audio_output: Option<String>,
    pub auto_translate_enabled: bool,
    pub default_speak_language: Option<String>,
    pub default_listen_language: Option<String>,
}
