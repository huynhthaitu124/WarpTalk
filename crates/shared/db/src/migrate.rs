use sqlx::{Connection, PgPool};

/// One forward-only migration. Never edit a migration that has been applied.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations of one module (tracked independently by module name).
#[derive(Debug, Clone, Copy)]
pub struct ModuleMigrations {
    pub module: &'static str,
    pub migrations: &'static [Migration],
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("migration checksum mismatch for module `{module}` version {version}")]
    ChecksumMismatch { module: String, version: i64 },
    #[error("duplicate migration version {version} in module `{module}`")]
    DuplicateVersion { module: String, version: i64 },
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

const LOCK_KEY_SEED: &str = "warptalk.schema_migrations";

/// FNV-1a 64-bit hash.
fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn checksum(sql: &str) -> String {
    format!("{:016x}", fnv1a(sql.as_bytes()))
}

/// Apply unapplied migrations of each module (in given order, version order within a module).
/// Serialised across processes by a session-level advisory lock.
pub async fn run_migrations(pool: &PgPool, sets: &[ModuleMigrations]) -> Result<(), DbError> {
    let mut conn = pool.acquire().await?;
    let key = fnv1a(LOCK_KEY_SEED.as_bytes()) as i64;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await?;

    let result = apply_all(&mut conn, sets).await;

    let unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await;
    if let Err(e) = unlock {
        tracing::warn!(error = %e, "failed to release migration advisory lock");
        // The connection state is unknown; drop it from the pool so the lock dies with it.
        let _ = conn.detach().close().await;
    }
    result
}

async fn apply_all(
    conn: &mut sqlx::PgConnection,
    sets: &[ModuleMigrations],
) -> Result<(), DbError> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS public.schema_migrations (
            module     text NOT NULL,
            version    bigint NOT NULL,
            name       text NOT NULL,
            checksum   text NOT NULL,
            applied_at timestamptz NOT NULL DEFAULT now(),
            PRIMARY KEY (module, version)
        )",
    )
    .execute(&mut *conn)
    .await?;

    for set in sets {
        let mut ordered: Vec<&Migration> = set.migrations.iter().collect();
        ordered.sort_by_key(|m| m.version);
        if let Some(w) = ordered.windows(2).find(|w| w[0].version == w[1].version) {
            return Err(DbError::DuplicateVersion {
                module: set.module.to_string(),
                version: w[0].version,
            });
        }

        let applied: Vec<(i64, String)> = sqlx::query_as(
            "SELECT version, checksum FROM public.schema_migrations WHERE module = $1",
        )
        .bind(set.module)
        .fetch_all(&mut *conn)
        .await?;

        for m in ordered {
            let sum = checksum(m.sql);
            if let Some((_, existing)) = applied.iter().find(|(v, _)| *v == m.version) {
                if *existing != sum {
                    return Err(DbError::ChecksumMismatch {
                        module: set.module.to_string(),
                        version: m.version,
                    });
                }
                continue;
            }
            let mut tx = conn.begin().await?;
            sqlx::raw_sql(m.sql).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO public.schema_migrations (module, version, name, checksum)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(set.module)
            .bind(m.version)
            .bind(m.name)
            .bind(&sum)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            tracing::info!(
                module = set.module,
                version = m.version,
                name = m.name,
                "applied migration"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_vectors() {
        assert_eq!(checksum(""), "cbf29ce484222325");
        assert_eq!(checksum("a"), "af63dc4c8601ec8c");
    }
}
