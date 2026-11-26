use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, UserId};

use crate::application::ports::{
    ClientInfo, IdentityRepository, NewRefreshToken, NewUser, RotateOutcome, SessionRecord,
    UserAuthRecord,
};
use crate::domain::{AccountFlags, Profile, Settings, UserSummary};

pub struct PgIdentityRepository {
    pool: PgPool,
}

impl PgIdentityRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const AUTH_COLS: &str = "id, email, password_hash, email_verified, is_active, is_locked, \
                         locked_until, (deleted_at IS NOT NULL) AS deleted";

fn auth_record(r: &PgRow) -> Result<UserAuthRecord, sqlx::Error> {
    Ok(UserAuthRecord {
        id: UserId::from_uuid(r.try_get("id")?),
        email: r.try_get("email")?,
        password_hash: r.try_get("password_hash")?,
        flags: AccountFlags {
            email_verified: r.try_get("email_verified")?,
            is_active: r.try_get("is_active")?,
            is_locked: r.try_get("is_locked")?,
            locked_until: r.try_get("locked_until")?,
            deleted: r.try_get("deleted")?,
        },
    })
}

fn summary(r: &PgRow) -> Result<UserSummary, sqlx::Error> {
    Ok(UserSummary {
        id: UserId::from_uuid(r.try_get("id")?),
        email: r.try_get("email")?,
        full_name: r.try_get("full_name")?,
        avatar_url: r.try_get("avatar_url")?,
    })
}

fn settings_row(r: &PgRow) -> Result<Settings, sqlx::Error> {
    Ok(Settings {
        default_audio_input: r.try_get("default_audio_input")?,
        default_audio_output: r.try_get("default_audio_output")?,
        auto_translate_enabled: r.try_get("auto_translate_enabled")?,
        default_speak_language: r.try_get("default_speak_language")?,
        default_listen_language: r.try_get("default_listen_language")?,
    })
}

#[async_trait::async_trait]
impl IdentityRepository for PgIdentityRepository {
    async fn insert_user(&self, u: NewUser) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let res = sqlx::query(
            "INSERT INTO auth.users (id, email, password_hash, full_name, email_verification_token_hash,
                                     email_verification_token_expires_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(u.id.as_uuid())
        .bind(&u.email)
        .bind(&u.password_hash)
        .bind(&u.full_name)
        .bind(&u.verify_token_hash)
        .bind(u.verify_expires_at)
        .bind(u.now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = res {
            if let sqlx::Error::Database(db) = &e
                && db.code().as_deref() == Some("23505")
            {
                return Err(AppError::conflict(
                    "EMAIL_TAKEN",
                    "email is already registered",
                ));
            }
            return Err(map_sqlx_error(e));
        }
        sqlx::query("INSERT INTO auth.user_settings (id, user_id, updated_at) VALUES ($1, $2, $3)")
            .bind(Uuid::now_v7())
            .bind(u.id.as_uuid())
            .bind(u.now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn find_auth_by_email(&self, email: &str) -> AppResult<Option<UserAuthRecord>> {
        let row = sqlx::query(&format!(
            "SELECT {AUTH_COLS} FROM auth.users WHERE email = $1"
        ))
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(auth_record)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn find_auth_by_id(&self, id: UserId) -> AppResult<Option<UserAuthRecord>> {
        let row = sqlx::query(&format!("SELECT {AUTH_COLS} FROM auth.users WHERE id = $1"))
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(auth_record)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn record_login_failure(
        &self,
        id: UserId,
        now: DateTime<Utc>,
        max_failures: i32,
        lock_until: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE auth.users u
                SET failed_login_attempts = n.attempts,
                    locked_until = CASE WHEN n.attempts >= $3 THEN $4 ELSE NULL END,
                    updated_at = $2
               FROM (SELECT id,
                            CASE WHEN locked_until IS NOT NULL AND locked_until <= $2
                                 THEN 1 ELSE failed_login_attempts + 1 END AS attempts
                       FROM auth.users WHERE id = $1 FOR UPDATE) n
              WHERE u.id = n.id",
        )
        .bind(id.as_uuid())
        .bind(now)
        .bind(max_failures)
        .bind(lock_until)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn record_login_success(&self, id: UserId, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE auth.users SET failed_login_attempts = 0, locked_until = NULL,
                    last_login_at = $2, updated_at = $2 WHERE id = $1",
        )
        .bind(id.as_uuid())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn consume_verification_token(&self, hash: &str, now: DateTime<Utc>) -> AppResult<bool> {
        let res = sqlx::query(
            "UPDATE auth.users
                SET email_verified = true, email_verified_at = $2,
                    email_verification_token_hash = NULL,
                    email_verification_token_expires_at = NULL, updated_at = $2
              WHERE email_verification_token_hash = $1
                AND email_verification_token_expires_at > $2
                AND deleted_at IS NULL",
        )
        .bind(hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(res.rows_affected() == 1)
    }

    async fn is_system_admin(&self, id: UserId) -> AppResult<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM auth.user_roles ur
                              JOIN auth.roles r ON r.id = ur.role_id
                             WHERE ur.user_id = $1 AND ur.revoked_at IS NULL
                               AND r.name = 'SYSTEM_ADMIN' AND r.is_active AND r.deleted_at IS NULL)",
        )
        .bind(id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)
    }

    async fn insert_refresh_token(&self, t: NewRefreshToken) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO auth.refresh_tokens
                    (id, user_id, token_hash, family_id, device_info, ip_address, expires_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(t.user_id.as_uuid())
        .bind(&t.token_hash)
        .bind(t.family_id)
        .bind(t.client.device_info.as_deref())
        .bind(t.client.ip_address.as_deref())
        .bind(t.expires_at)
        .bind(t.now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn rotate_refresh_token(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
        client: &ClientInfo,
    ) -> AppResult<RotateOutcome> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let row = sqlx::query(
            "SELECT user_id, family_id, expires_at, revoked_at, device_info, ip_address
               FROM auth.refresh_tokens WHERE token_hash = $1 FOR UPDATE",
        )
        .bind(old_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let Some(row) = row else {
            return Ok(RotateOutcome::Unknown);
        };
        let user_id: Uuid = row.try_get("user_id").map_err(map_sqlx_error)?;
        let family_id: Uuid = row.try_get("family_id").map_err(map_sqlx_error)?;
        let expires_at: DateTime<Utc> = row.try_get("expires_at").map_err(map_sqlx_error)?;
        let revoked_at: Option<DateTime<Utc>> =
            row.try_get("revoked_at").map_err(map_sqlx_error)?;
        let old_device: Option<String> = row.try_get("device_info").map_err(map_sqlx_error)?;
        let old_ip: Option<String> = row.try_get("ip_address").map_err(map_sqlx_error)?;

        if revoked_at.is_some() {
            revoke_family_tx(&mut tx, family_id, now).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            return Ok(RotateOutcome::Reused);
        }
        if expires_at <= now {
            return Ok(RotateOutcome::Expired);
        }
        sqlx::query("UPDATE auth.refresh_tokens SET revoked_at = $2 WHERE token_hash = $1")
            .bind(old_hash)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO auth.refresh_tokens
                    (id, user_id, token_hash, family_id, device_info, ip_address, expires_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(new_hash)
        .bind(family_id)
        // The session keeps describing the device it was signed in on; the address follows the
        // client as it moves between networks.
        .bind(client.device_info.clone().or(old_device))
        .bind(client.ip_address.clone().or(old_ip))
        .bind(new_expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        tx.commit().await.map_err(map_sqlx_error)?;
        Ok(RotateOutcome::Rotated {
            user_id: UserId::from_uuid(user_id),
            family_id,
        })
    }

    async fn revoke_family_by_token(&self, hash: &str, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE auth.refresh_tokens SET revoked_at = $2
              WHERE revoked_at IS NULL
                AND family_id = (SELECT family_id FROM auth.refresh_tokens WHERE token_hash = $1)",
        )
        .bind(hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn revoke_family(&self, family_id: Uuid, now: DateTime<Utc>) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        revoke_family_tx(&mut tx, family_id, now).await?;
        tx.commit().await.map_err(map_sqlx_error)
    }

    async fn list_sessions(
        &self,
        user: UserId,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<SessionRecord>> {
        // A family is live while it holds an unrevoked, unexpired token (rotation keeps exactly
        // one). Its first token is the sign-in; its live token the latest refresh.
        let rows = sqlx::query(
            "SELECT live.family_id, live.device_info, live.ip_address, live.expires_at,
                    live.created_at AS last_active_at, fam.signed_in_at
               FROM auth.refresh_tokens live
               JOIN (SELECT family_id, MIN(created_at) AS signed_in_at
                       FROM auth.refresh_tokens WHERE user_id = $1 GROUP BY family_id) fam
                 ON fam.family_id = live.family_id
              WHERE live.user_id = $1 AND live.revoked_at IS NULL AND live.expires_at > $2
              ORDER BY live.created_at DESC",
        )
        .bind(user.as_uuid())
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        rows.iter()
            .map(|r| {
                Ok(SessionRecord {
                    family_id: r.try_get("family_id").map_err(map_sqlx_error)?,
                    device_info: r.try_get("device_info").map_err(map_sqlx_error)?,
                    ip_address: r.try_get("ip_address").map_err(map_sqlx_error)?,
                    signed_in_at: r.try_get("signed_in_at").map_err(map_sqlx_error)?,
                    last_active_at: r.try_get("last_active_at").map_err(map_sqlx_error)?,
                    expires_at: r.try_get("expires_at").map_err(map_sqlx_error)?,
                })
            })
            .collect()
    }

    async fn revoke_user_family(
        &self,
        user: UserId,
        family_id: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let done = sqlx::query(
            "UPDATE auth.refresh_tokens SET revoked_at = $3
              WHERE user_id = $1 AND family_id = $2 AND revoked_at IS NULL AND expires_at > $3",
        )
        .bind(user.as_uuid())
        .bind(family_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(done.rows_affected() > 0)
    }

    async fn revoke_other_families(
        &self,
        user: UserId,
        keep: Uuid,
        now: DateTime<Utc>,
    ) -> AppResult<u64> {
        let done = sqlx::query(
            "UPDATE auth.refresh_tokens SET revoked_at = $3
              WHERE user_id = $1 AND family_id <> $2 AND revoked_at IS NULL",
        )
        .bind(user.as_uuid())
        .bind(keep)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(done.rows_affected())
    }

    async fn get_profile(&self, id: UserId) -> AppResult<Option<Profile>> {
        let row = sqlx::query(
            "SELECT full_name, avatar_url, preferred_language, timezone
               FROM auth.users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.map(|r| {
            Ok(Profile {
                full_name: r.try_get("full_name")?,
                avatar_url: r.try_get("avatar_url")?,
                preferred_language: r.try_get("preferred_language")?,
                timezone: r.try_get("timezone")?,
            })
        })
        .transpose()
        .map_err(map_sqlx_error)
    }

    async fn update_profile(&self, id: UserId, p: &Profile, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE auth.users SET full_name = $2, avatar_url = $3, preferred_language = $4,
                    timezone = $5, updated_at = $6, updated_by = $1
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
        .bind(&p.full_name)
        .bind(&p.avatar_url)
        .bind(&p.preferred_language)
        .bind(&p.timezone)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn get_settings(&self, id: UserId, now: DateTime<Utc>) -> AppResult<Settings> {
        sqlx::query(
            "INSERT INTO auth.user_settings (id, user_id, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(id.as_uuid())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let row = sqlx::query(
            "SELECT default_audio_input, default_audio_output, auto_translate_enabled,
                    default_speak_language, default_listen_language
               FROM auth.user_settings WHERE user_id = $1",
        )
        .bind(id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        settings_row(&row).map_err(map_sqlx_error)
    }

    async fn update_settings(&self, id: UserId, s: &Settings, now: DateTime<Utc>) -> AppResult<()> {
        sqlx::query(
            "UPDATE auth.user_settings
                SET default_audio_input = $2, default_audio_output = $3,
                    auto_translate_enabled = $4, default_speak_language = $5,
                    default_listen_language = $6, updated_at = $7, updated_by = $1
              WHERE user_id = $1",
        )
        .bind(id.as_uuid())
        .bind(&s.default_audio_input)
        .bind(&s.default_audio_output)
        .bind(s.auto_translate_enabled)
        .bind(&s.default_speak_language)
        .bind(&s.default_listen_language)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn find_summary_by_id(&self, id: UserId) -> AppResult<Option<UserSummary>> {
        let row = sqlx::query(
            "SELECT id, email, full_name, avatar_url FROM auth.users
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(summary)
            .transpose()
            .map_err(map_sqlx_error)
    }

    async fn find_summary_by_email(&self, email: &str) -> AppResult<Option<UserSummary>> {
        let row = sqlx::query(
            "SELECT id, email, full_name, avatar_url FROM auth.users
              WHERE email = $1 AND deleted_at IS NULL",
        )
        .bind(email.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        row.as_ref()
            .map(summary)
            .transpose()
            .map_err(map_sqlx_error)
    }
}

async fn revoke_family_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    family_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE auth.refresh_tokens SET revoked_at = $2 WHERE family_id = $1 AND revoked_at IS NULL",
    )
    .bind(family_id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}
