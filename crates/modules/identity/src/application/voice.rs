//! Voice consent and enrollment use cases (spec 13 sections 5-8, 11; BR-94).
//!
//! Consent is the hard precondition for cloning. Every change is mirrored to the runtime
//! (`VoiceStatePublisher`, Redis `voice:consent:{userId}`) so the pipeline can re-check consent
//! per utterance without a database.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult, Clock, UserId};

use crate::domain::voice::{self, Sample, SampleError};

pub const CONSENT_REQUIRED: &str = "VOICE_CONSENT_REQUIRED";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentView {
    pub version: String,
    pub granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct VoiceProfileView {
    pub id: Uuid,
    pub status: String,
    pub language: Option<String>,
    pub quality_score: Option<i32>,
    pub provider_voice_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewConsent {
    pub user: UserId,
    pub version: String,
    pub text_hash: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewEnrollment {
    pub user: UserId,
    pub display_name: Option<String>,
    pub language: String,
    pub provider: Option<String>,
    /// `None` when the provider clone failed (profile is stored as `clone_failed`).
    pub provider_voice_id: Option<String>,
    pub clone_error_code: Option<String>,
    pub quality_score: i32,
    pub duration_seconds: i32,
    pub now: DateTime<Utc>,
}

/// What revocation removed; provider voices still need deleting.
#[derive(Debug, Clone, Default)]
pub struct RevokeOutcome {
    pub was_granted: bool,
    pub provider_voice_ids: Vec<String>,
    pub samples_deleted: u64,
}

/// State mirrored to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRuntimeState {
    pub consent: bool,
    pub provider_voice_id: Option<String>,
}

#[async_trait::async_trait]
pub trait VoiceRepository: Send + Sync {
    async fn active_consent(&self, user: UserId) -> AppResult<Option<ConsentView>>;
    /// Idempotent: an already active consent is returned unchanged.
    async fn grant_consent(&self, c: NewConsent) -> AppResult<ConsentView>;
    /// One transaction: consent -> REVOKED, profiles and samples soft-deleted (file refs
    /// cleared), provider voices queued in `provider_voice_deletions`.
    async fn revoke_consent(&self, user: UserId, now: DateTime<Utc>) -> AppResult<RevokeOutcome>;
    async fn active_profile(&self, user: UserId) -> AppResult<Option<VoiceProfileView>>;
    /// Replaces the user's current profile; returns the replaced provider voice ids (queued for
    /// deletion).
    async fn replace_profile(&self, e: NewEnrollment) -> AppResult<(Uuid, Vec<String>)>;
    async fn pending_deletions(&self, limit: i64) -> AppResult<Vec<String>>;
    async fn complete_deletion(&self, provider_voice_id: &str, now: DateTime<Utc>)
    -> AppResult<()>;
}

/// Deletes a voice at the provider (spec 13 section 11).
#[async_trait::async_trait]
pub trait VoiceProviderAdmin: Send + Sync {
    async fn delete_voice(&self, provider_voice_id: &str) -> AppResult<()>;
}

/// Creates a provider voice from a mono 16-bit WAV; returns the provider voice id.
#[async_trait::async_trait]
pub trait VoiceCloner: Send + Sync {
    fn provider(&self) -> &'static str;
    async fn clone_voice(&self, wav: &[u8], name: &str, language: &str) -> AppResult<String>;
}

#[async_trait::async_trait]
pub trait VoiceStatePublisher: Send + Sync {
    async fn publish(&self, user: UserId, state: &VoiceRuntimeState) -> AppResult<()>;
}

pub struct NoopVoiceProviderAdmin;

#[async_trait::async_trait]
impl VoiceProviderAdmin for NoopVoiceProviderAdmin {
    async fn delete_voice(&self, _: &str) -> AppResult<()> {
        Ok(())
    }
}

/// No provider configured: enrollment answers 503 `VOICE_CLONING_UNAVAILABLE`.
pub struct NoopVoiceCloner;

#[async_trait::async_trait]
impl VoiceCloner for NoopVoiceCloner {
    fn provider(&self) -> &'static str {
        "none"
    }
    async fn clone_voice(&self, _: &[u8], _: &str, _: &str) -> AppResult<String> {
        Err(AppError::Coded {
            status: 503,
            code: "VOICE_CLONING_UNAVAILABLE".to_owned(),
            message: "voice cloning is not configured".to_owned(),
        })
    }
}

pub struct NoopVoiceStatePublisher;

#[async_trait::async_trait]
impl VoiceStatePublisher for NoopVoiceStatePublisher {
    async fn publish(&self, _: UserId, _: &VoiceRuntimeState) -> AppResult<()> {
        Ok(())
    }
}

/// Provider-side ports of the voice feature (all no-ops by default).
#[derive(Clone)]
pub struct VoiceDeps {
    pub admin: Arc<dyn VoiceProviderAdmin>,
    pub cloner: Arc<dyn VoiceCloner>,
    pub publisher: Arc<dyn VoiceStatePublisher>,
}

impl Default for VoiceDeps {
    fn default() -> Self {
        Self {
            admin: Arc::new(NoopVoiceProviderAdmin),
            cloner: Arc::new(NoopVoiceCloner),
            publisher: Arc::new(NoopVoiceStatePublisher),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VoiceStatus {
    pub consent: Option<ConsentView>,
    pub profile: Option<VoiceProfileView>,
}

impl VoiceStatus {
    pub fn clone_enabled(&self) -> bool {
        self.consent.is_some()
            && self
                .profile
                .as_ref()
                .is_some_and(|p| p.provider_voice_id.is_some())
    }
}

pub struct SampleInput {
    pub bytes: Vec<u8>,
    /// `None`: WAV container. `Some(rate)`: raw mono 16-bit little-endian PCM.
    pub pcm_rate: Option<u32>,
    pub language: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnrollResult {
    pub profile_id: Uuid,
    pub quality_score: i32,
    pub duration_seconds: f32,
    pub language: String,
}

pub struct VoiceService {
    repo: Arc<dyn VoiceRepository>,
    deps: VoiceDeps,
    clock: Arc<dyn Clock>,
}

fn sample_err(e: SampleError) -> AppError {
    AppError::validation(e.code(), e.message())
}

impl VoiceService {
    pub fn new(repo: Arc<dyn VoiceRepository>, deps: VoiceDeps, clock: Arc<dyn Clock>) -> Self {
        Self { repo, deps, clock }
    }

    pub async fn status(&self, user: UserId) -> AppResult<VoiceStatus> {
        let consent = self.repo.active_consent(user).await?;
        let profile = if consent.is_some() {
            self.repo.active_profile(user).await?
        } else {
            None
        };
        Ok(VoiceStatus { consent, profile })
    }

    async fn publish(&self, user: UserId) -> AppResult<()> {
        let st = self.status(user).await?;
        let state = VoiceRuntimeState {
            consent: st.consent.is_some(),
            provider_voice_id: st.profile.and_then(|p| p.provider_voice_id),
        };
        self.deps.publisher.publish(user, &state).await
    }

    /// Records who/when/which text version (audit row) and publishes the runtime state. A
    /// publish failure is an error so the client retries (the grant itself is idempotent).
    pub async fn grant(
        &self,
        user: UserId,
        version: &str,
        text_hash: &str,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> AppResult<VoiceStatus> {
        let version = version.trim();
        if version.is_empty() || version.chars().count() > 40 {
            return Err(AppError::validation(
                "VALIDATION_ERROR",
                "consentVersion must be 1-40 characters",
            ));
        }
        let hash = voice::normalise_hash(text_hash).ok_or_else(|| {
            AppError::validation(
                "VALIDATION_ERROR",
                "acknowledgementHash must be a SHA-256 hex digest",
            )
        })?;
        self.repo
            .grant_consent(NewConsent {
                user,
                version: version.to_owned(),
                text_hash: hash,
                ip,
                user_agent,
                now: self.clock.now(),
            })
            .await?;
        tracing::info!(user = %user, version, "voice clone consent granted");
        self.publish(user).await?;
        self.status(user).await
    }

    /// Revocation (spec 13 section 11): cloning is disabled first (DB + runtime), stored
    /// samples are deleted, provider voices are deleted best effort and stay queued when the
    /// provider fails. Idempotent: a repeat call re-publishes the (revoked) runtime state.
    pub async fn revoke(&self, user: UserId) -> AppResult<VoiceStatus> {
        let out = self.repo.revoke_consent(user, self.clock.now()).await?;
        tracing::info!(
            user = %user,
            was_granted = out.was_granted,
            samples_deleted = out.samples_deleted,
            provider_voices = out.provider_voice_ids.len(),
            "voice clone consent revoked"
        );
        self.publish(user).await?;
        self.delete_at_provider(&out.provider_voice_ids).await;
        self.status(user).await
    }

    async fn delete_at_provider(&self, ids: &[String]) {
        for id in ids {
            match self.deps.admin.delete_voice(id).await {
                Ok(()) => {
                    if let Err(e) = self.repo.complete_deletion(id, self.clock.now()).await {
                        tracing::warn!(error = %e, "recording provider voice deletion failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "provider voice deletion failed; left pending");
                }
            }
        }
    }

    /// Retries provider deletions that failed earlier. Returns how many completed.
    pub async fn retry_pending_deletions(&self, limit: i64) -> AppResult<usize> {
        let ids = self.repo.pending_deletions(limit).await?;
        let mut done = 0;
        for id in &ids {
            if self.deps.admin.delete_voice(id).await.is_ok() {
                self.repo.complete_deletion(id, self.clock.now()).await?;
                done += 1;
            }
        }
        Ok(done)
    }

    pub async fn enroll(&self, user: UserId, input: SampleInput) -> AppResult<EnrollResult> {
        if self.repo.active_consent(user).await?.is_none() {
            return Err(AppError::Coded {
                status: 403,
                code: CONSENT_REQUIRED.to_owned(),
                message: "voice clone consent is required".to_owned(),
            });
        }
        let sample: Sample = match input.pcm_rate {
            Some(rate) => voice::decode_pcm16(&input.bytes, rate),
            None => voice::decode_wav(&input.bytes),
        }
        .map_err(sample_err)?;
        let assessment = voice::assess(&sample).map_err(sample_err)?;
        let name = input
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(100).collect::<String>());
        let clone_name = format!("wt-{}", user.as_uuid().simple());
        let wav = sample.to_wav();
        let cloned = self
            .deps
            .cloner
            .clone_voice(&wav, &clone_name, &input.language)
            .await;
        let (voice_id, err_code) = match cloned {
            Ok(id) => (Some(id), None),
            // Not configured: nothing was attempted, store nothing.
            Err(e @ AppError::Coded { status: 503, .. }) => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "voice clone creation failed");
                (None, Some("CLONE_FAILED".to_owned()))
            }
        };
        let (profile_id, replaced) = self
            .repo
            .replace_profile(NewEnrollment {
                user,
                display_name: name,
                language: input.language.clone(),
                provider: Some(self.deps.cloner.provider().to_owned()),
                provider_voice_id: voice_id.clone(),
                clone_error_code: err_code,
                quality_score: assessment.quality_score,
                duration_seconds: assessment.duration_seconds.round() as i32,
                now: self.clock.now(),
            })
            .await?;
        self.delete_at_provider(&replaced).await;
        if voice_id.is_none() {
            return Err(AppError::Coded {
                status: 502,
                code: "VOICE_CLONE_FAILED".to_owned(),
                message: "voice clone creation failed".to_owned(),
            });
        }
        if let Err(e) = self.publish(user).await {
            tracing::warn!(error = %e, "publishing voice state failed");
        }
        Ok(EnrollResult {
            profile_id,
            quality_score: assessment.quality_score,
            duration_seconds: assessment.duration_seconds,
            language: input.language,
        })
    }

    pub async fn voice_clone_consent(&self, user: UserId) -> AppResult<bool> {
        Ok(self.repo.active_consent(user).await?.is_some())
    }

    /// The provider voice, only while consent is active.
    pub async fn provider_voice(&self, user: UserId) -> AppResult<Option<String>> {
        Ok(self
            .status(user)
            .await?
            .profile
            .and_then(|p| p.provider_voice_id))
    }
}
