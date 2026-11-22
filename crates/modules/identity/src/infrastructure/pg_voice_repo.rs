use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppResult, UserId};

use crate::application::voice::{
    ConsentView, NewConsent, NewEnrollment, RevokeOutcome, VoiceProfileView, VoiceRepository,
};

pub struct PgVoiceRepository {
    pool: PgPool,
}

impl PgVoiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn consent_view(r: &PgRow) -> Result<ConsentView, sqlx::Error> {
    Ok(ConsentView {
        version: r.try_get("consent_text_version")?,
        granted_at: r.try_get("granted_at")?,
    })
}

fn profile_view(r: &PgRow) -> Result<VoiceProfileView, sqlx::Error> {
    Ok(VoiceProfileView {
        id: r.try_get("id")?,
        status: r.try_get("status")?,
        language: r.try_get("language")?,
        quality_score: r.try_get("quality_score")?,
        provider_voice_id: r.try_get("embedding_ref")?,
        created_at: r.try_get("created_at")?,
    })
}

const ACTIVE_CONSENT: &str = "SELECT consent_text_version, granted_at FROM voice.voice_consents \
     WHERE user_id = $1 AND consent_type = 'VOICE_CLONE' AND consent_status = 'GRANTED'";

#[async_trait::async_trait]
impl VoiceRepository for PgVoiceRepository {
    async fn active_consent(&self, user: UserId) -> AppResult<Option<ConsentView>> {
        let row = sqlx::query(ACTIVE_CONSENT)
            .bind(user.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(consent_view)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn grant_consent(&self, c: NewConsent) -> AppResult<ConsentView> {
        sqlx::query(
            "INSERT INTO voice.voice_consents \
             (id, user_id, consent_type, consent_status, consent_text_version, consent_text_hash, \
              granted_at, ip_address, user_agent, created_at) \
             VALUES ($1, $2, 'VOICE_CLONE', 'GRANTED', $3, $4, $5, $6, $7, $5) \
             ON CONFLICT (user_id, consent_type) WHERE consent_status = 'GRANTED' DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(c.user.as_uuid())
        .bind(&c.version)
        .bind(&c.text_hash)
        .bind(c.now)
        .bind(
            c.ip.as_deref()
                .map(|s| s.chars().take(45).collect::<String>()),
        )
        .bind(
            c.user_agent
                .as_deref()
                .map(|s| s.chars().take(255).collect::<String>()),
        )
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let row = sqlx::query(ACTIVE_CONSENT)
            .bind(c.user.as_uuid())
            .fetch_one(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        consent_view(&row).map_err(map_sqlx_error)
    }

    async fn revoke_consent(&self, user: UserId, now: DateTime<Utc>) -> AppResult<RevokeOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let revoked = sqlx::query(
            "UPDATE voice.voice_consents SET consent_status = 'REVOKED', revoked_at = $2 \
             WHERE user_id = $1 AND consent_type = 'VOICE_CLONE' AND consent_status = 'GRANTED'",
        )
        .bind(user.as_uuid())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        let profiles = sqlx::query(
            "UPDATE voice.voice_profiles SET deleted_at = $2, deleted_by = $1, is_active = false, \
             status = 'deleted', updated_at = $2, updated_by = $1 \
             WHERE user_id = $1 AND deleted_at IS NULL \
             RETURNING id, embedding_ref",
        )
        .bind(user.as_uuid())
        .bind(now)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let ids: Vec<Uuid> = profiles
            .iter()
            .map(|r| r.try_get("id"))
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)?;
        let voices: Vec<String> = profiles
            .iter()
            .filter_map(|r| r.try_get::<Option<String>, _>("embedding_ref").transpose())
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)?;
        let samples = sqlx::query(
            "UPDATE voice.voice_samples SET deleted_at = $2, deleted_by = $3, file_url = NULL, \
             contains_raw_audio = false \
             WHERE voice_profile_id = ANY($1) AND deleted_at IS NULL",
        )
        .bind(&ids)
        .bind(now)
        .bind(user.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?
        .rows_affected();
        for v in &voices {
            queue_deletion(&mut tx, user, v, "consent-revoked", now).await?;
        }
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(RevokeOutcome {
            was_granted: revoked > 0,
            provider_voice_ids: voices,
            samples_deleted: samples,
        })
    }

    async fn active_profile(&self, user: UserId) -> AppResult<Option<VoiceProfileView>> {
        let row = sqlx::query(
            "SELECT id, status, language, quality_score, embedding_ref, created_at \
             FROM voice.voice_profiles \
             WHERE user_id = $1 AND deleted_at IS NULL AND is_active \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(user.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(profile_view)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn replace_profile(&self, e: NewEnrollment) -> AppResult<(Uuid, Vec<String>)> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let old = sqlx::query(
            "UPDATE voice.voice_profiles SET deleted_at = $2, deleted_by = $1, is_active = false, \
             status = 'replaced', updated_at = $2, updated_by = $1 \
             WHERE user_id = $1 AND deleted_at IS NULL RETURNING id, embedding_ref",
        )
        .bind(e.user.as_uuid())
        .bind(e.now)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let old_ids: Vec<Uuid> = old
            .iter()
            .map(|r| r.try_get("id"))
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)?;
        let old_voices: Vec<String> = old
            .iter()
            .filter_map(|r| r.try_get::<Option<String>, _>("embedding_ref").transpose())
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)?;
        sqlx::query(
            "UPDATE voice.voice_samples SET deleted_at = $2, deleted_by = $3, file_url = NULL \
             WHERE voice_profile_id = ANY($1) AND deleted_at IS NULL",
        )
        .bind(&old_ids)
        .bind(e.now)
        .bind(e.user.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        for v in &old_voices {
            queue_deletion(&mut tx, e.user, v, "replaced", e.now).await?;
        }
        let profile_id = Uuid::now_v7();
        let status = if e.provider_voice_id.is_some() {
            "active"
        } else {
            "clone_failed"
        };
        sqlx::query(
            "INSERT INTO voice.voice_profiles \
             (id, user_id, display_name, provider, embedding_ref, language, status, is_active, \
              created_at, created_by, source, quality_score, clone_error_code) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, true, $8, $2, 'upload', $9, $10)",
        )
        .bind(profile_id)
        .bind(e.user.as_uuid())
        .bind(&e.display_name)
        .bind(&e.provider)
        .bind(&e.provider_voice_id)
        .bind(&e.language)
        .bind(status)
        .bind(e.now)
        .bind(e.quality_score)
        .bind(&e.clone_error_code)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        // Metadata only: the audio itself is not retained (no file_url, no raw audio).
        sqlx::query(
            "INSERT INTO voice.voice_samples \
             (id, voice_profile_id, sample_type, duration_seconds, language, contains_raw_audio, \
              created_at) VALUES ($1, $2, 'upload', $3, $4, false, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(profile_id)
        .bind(e.duration_seconds)
        .bind(&e.language)
        .bind(e.now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok((profile_id, old_voices))
    }

    async fn pending_deletions(&self, limit: i64) -> AppResult<Vec<String>> {
        let rows = sqlx::query(
            "SELECT provider_voice_id FROM voice.provider_voice_deletions \
             WHERE completed_at IS NULL ORDER BY requested_at LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(|r| r.try_get("provider_voice_id"))
            .collect::<Result<_, _>>()
            .map_err(map_sqlx_error)
    }

    async fn complete_deletion(&self, id: &str, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE voice.provider_voice_deletions SET completed_at = $2 \
             WHERE provider_voice_id = $1 AND completed_at IS NULL",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }
}

async fn queue_deletion(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: UserId,
    voice_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO voice.provider_voice_deletions \
         (provider_voice_id, user_id, reason, requested_at) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (provider_voice_id) DO NOTHING",
    )
    .bind(voice_id)
    .bind(user.as_uuid())
    .bind(reason)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}
