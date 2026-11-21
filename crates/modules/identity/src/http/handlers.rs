use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use warptalk_kernel::AppResult;
use warptalk_web::AuthUser;

use super::HttpState;
use super::dto::*;
use crate::application::{RegisterCommand, SettingsUpdate};

pub async fn register(
    State(s): State<HttpState>,
    Json(b): Json<RegisterRequest>,
) -> AppResult<(StatusCode, Json<RegisterResponse>)> {
    let user_id = s
        .auth
        .register(RegisterCommand {
            email: b.email,
            password: b.password,
            full_name: b.full_name,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(RegisterResponse { user_id })))
}

pub async fn verify_email(
    State(s): State<HttpState>,
    Json(b): Json<VerifyEmailRequest>,
) -> AppResult<StatusCode> {
    s.auth.verify_email(&b.token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn login(
    State(s): State<HttpState>,
    Json(b): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    Ok(Json(s.auth.login(&b.email, &b.password).await?.into()))
}

pub async fn refresh(
    State(s): State<HttpState>,
    Json(b): Json<RefreshRequest>,
) -> AppResult<Json<AuthResponse>> {
    Ok(Json(s.auth.refresh(&b.refresh_token).await?.into()))
}

pub async fn logout(
    State(s): State<HttpState>,
    Json(b): Json<RefreshRequest>,
) -> AppResult<StatusCode> {
    s.auth.logout(&b.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_profile(State(s): State<HttpState>, u: AuthUser) -> AppResult<Json<ProfileDto>> {
    Ok(Json(s.profile.get_profile(u.user_id()).await?.into()))
}

pub async fn put_profile(
    State(s): State<HttpState>,
    u: AuthUser,
    Json(b): Json<ProfileDto>,
) -> AppResult<Json<ProfileDto>> {
    let p = s
        .profile
        .update_profile(
            u.user_id(),
            &b.full_name,
            b.avatar_url.as_deref(),
            &b.preferred_language,
            b.timezone.as_deref(),
        )
        .await?;
    Ok(Json(p.into()))
}

pub async fn get_settings(State(s): State<HttpState>, u: AuthUser) -> AppResult<Json<SettingsDto>> {
    Ok(Json(s.profile.get_settings(u.user_id()).await?.into()))
}

pub async fn put_settings(
    State(s): State<HttpState>,
    u: AuthUser,
    Json(b): Json<SettingsDto>,
) -> AppResult<Json<SettingsDto>> {
    let out = s
        .profile
        .update_settings(
            u.user_id(),
            SettingsUpdate {
                default_audio_input: b.default_audio_input,
                default_audio_output: b.default_audio_output,
                auto_translate_enabled: b.auto_translate_enabled,
                default_speak_language: b.default_speak_language,
                default_listen_language: b.default_listen_language,
            },
        )
        .await?;
    Ok(Json(out.into()))
}

pub async fn me(State(s): State<HttpState>, u: AuthUser) -> AppResult<Json<MeDto>> {
    Ok(Json(s.profile.me(u.user_id()).await?.into()))
}
