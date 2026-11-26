use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use base64::Engine;
use warptalk_kernel::{AppError, AppResult};
use warptalk_web::AuthUser;
use warptalk_web::extract::Json;

use super::HttpState;
use super::dto::*;
use crate::application::ports::ClientInfo;
use crate::application::voice::SampleInput;
use crate::application::{RegisterCommand, SettingsUpdate};
use crate::domain::validation;

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

/// What the request says about the client, for the user's sessions page. Display only: the
/// forwarding headers are client-controllable when no proxy strips them, so nothing here may be
/// used for an access decision.
fn client_info(h: &HeaderMap) -> ClientInfo {
    let header = |name: &str| h.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    let device_info = header("user-agent")
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(255).collect());
    let ip_address = header("x-forwarded-for")
        .and_then(|v| v.split(',').next())
        .or_else(|| header("x-real-ip"))
        .and_then(|v| v.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
        .filter(|ip| ip.len() <= 45);
    ClientInfo {
        device_info,
        ip_address,
    }
}

/// Always 202 for a well-formed email (no account probing).
pub async fn resend_verification(
    State(s): State<HttpState>,
    Json(b): Json<EmailRequest>,
) -> AppResult<StatusCode> {
    s.auth.resend_verification(&b.email).await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn forgot_password(
    State(s): State<HttpState>,
    Json(b): Json<EmailRequest>,
) -> AppResult<StatusCode> {
    s.auth.forgot_password(&b.email).await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn reset_password(
    State(s): State<HttpState>,
    Json(b): Json<ResetPasswordRequest>,
) -> AppResult<StatusCode> {
    s.auth.reset_password(&b.token, &b.new_password).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn change_password(
    State(s): State<HttpState>,
    u: AuthUser,
    Json(b): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    s.auth
        .change_password(u.user_id(), u.0.sid, &b.current_password, &b.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn google_login(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Json(b): Json<GoogleTokenRequest>,
) -> AppResult<Json<AuthResponse>> {
    Ok(Json(
        s.auth
            .google_login(&b.id_token, client_info(&headers))
            .await?
            .into(),
    ))
}

pub async fn link_google(
    State(s): State<HttpState>,
    u: AuthUser,
    Json(b): Json<GoogleTokenRequest>,
) -> AppResult<StatusCode> {
    s.auth.link_google(u.user_id(), &b.id_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unlink_google(State(s): State<HttpState>, u: AuthUser) -> AppResult<StatusCode> {
    s.auth.unlink_google(u.user_id()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn login(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Json(b): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let client = client_info(&headers);
    Ok(Json(
        s.auth.login(&b.email, &b.password, client).await?.into(),
    ))
}

pub async fn refresh(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Json(b): Json<RefreshRequest>,
) -> AppResult<Json<AuthResponse>> {
    let client = client_info(&headers);
    Ok(Json(s.auth.refresh(&b.refresh_token, client).await?.into()))
}

pub async fn list_sessions(
    State(s): State<HttpState>,
    u: AuthUser,
) -> AppResult<Json<Vec<UserSessionDto>>> {
    let current = u.0.sid;
    let list = s.auth.sessions(u.user_id()).await?;
    Ok(Json(
        list.into_iter()
            .map(|r| UserSessionDto::new(r, current))
            .collect(),
    ))
}

/// Signs out another of the caller's sessions. The current one ends through `/auth/logout`, which
/// also clears the client's tokens.
pub async fn revoke_session(
    State(s): State<HttpState>,
    u: AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> AppResult<StatusCode> {
    if id == u.0.sid {
        return Err(AppError::coded(
            400,
            "CURRENT_SESSION",
            "use sign out to end the current session",
        ));
    }
    s.auth.revoke_session(u.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_other_sessions(
    State(s): State<HttpState>,
    u: AuthUser,
) -> AppResult<StatusCode> {
    s.auth.revoke_other_sessions(u.user_id(), u.0.sid).await?;
    Ok(StatusCode::NO_CONTENT)
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

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub async fn get_voice(State(s): State<HttpState>, u: AuthUser) -> AppResult<Json<VoiceStatusDto>> {
    Ok(Json(s.voice.status(u.user_id()).await?.into()))
}

pub async fn grant_voice_consent(
    State(s): State<HttpState>,
    u: AuthUser,
    headers: HeaderMap,
    Json(b): Json<GrantConsentRequest>,
) -> AppResult<Json<VoiceStatusDto>> {
    if !b.granted {
        return Err(AppError::validation(
            "VALIDATION_ERROR",
            "granted must be true (use DELETE to revoke)",
        ));
    }
    let ip = header(&headers, "x-forwarded-for")
        .map(|v| v.split(',').next().unwrap_or("").trim().to_owned())
        .filter(|v| !v.is_empty());
    let out = s
        .voice
        .grant(
            u.user_id(),
            &b.consent_version,
            &b.acknowledgement_hash,
            ip,
            header(&headers, "user-agent"),
        )
        .await?;
    Ok(Json(out.into()))
}

pub async fn revoke_voice_consent(
    State(s): State<HttpState>,
    u: AuthUser,
) -> AppResult<Json<VoiceStatusDto>> {
    Ok(Json(s.voice.revoke(u.user_id()).await?.into()))
}

pub async fn enroll_voice(
    State(s): State<HttpState>,
    u: AuthUser,
    Json(b): Json<EnrollRequest>,
) -> AppResult<Json<EnrollResponse>> {
    let bad = |m: &str| AppError::validation("VOICE_SAMPLE_INVALID", m);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b.sample_base64.trim())
        .map_err(|_| bad("sampleBase64 is not valid base64"))?;
    let pcm_rate = match b.format.as_deref().unwrap_or("wav") {
        "wav" => None,
        "pcm16" | "pcm" => Some(
            b.sample_rate
                .ok_or_else(|| bad("sampleRate is required for pcm16"))?,
        ),
        _ => return Err(bad("format must be wav or pcm16")),
    };
    let language = match b
        .language
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        Some(l) => {
            validation::language_tag(l).map_err(|m| AppError::validation("VALIDATION_ERROR", m))?
        }
        None => s.profile.get_profile(u.user_id()).await?.preferred_language,
    };
    let r = s
        .voice
        .enroll(
            u.user_id(),
            SampleInput {
                bytes,
                pcm_rate,
                language,
                display_name: b.display_name,
            },
        )
        .await?;
    Ok(Json(EnrollResponse {
        profile_id: r.profile_id,
        status: "active",
        quality_score: r.quality_score,
        duration_seconds: r.duration_seconds,
        language: r.language,
    }))
}
