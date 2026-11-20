#![allow(clippy::unwrap_used)]

use sqlx::PgPool;
use warptalk_db::{
    DbError, Migration, ModuleMigrations, OUTBOX_MIGRATIONS, claim_outbox_batch, connect,
    enqueue_outbox, mark_failed, mark_published, run_migrations,
};

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        println!("DATABASE_URL unset; skipping");
        return None;
    };
    Some(connect(&url, 5).await.unwrap())
}

#[tokio::test]
async fn migrations_idempotent_and_checksum() {
    let Some(pool) = pool().await else { return };
    let module: &'static str =
        Box::leak(format!("t_{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let table = format!("public.{module}_t");
    let sql1: &'static str = Box::leak(
        format!("CREATE TABLE {table} (id int); INSERT INTO {table} VALUES (1);").into_boxed_str(),
    );
    let migs: &'static [Migration] = Box::leak(Box::new([Migration {
        version: 1,
        name: "init",
        sql: sql1,
    }]));
    let set = ModuleMigrations {
        module,
        migrations: migs,
    };

    run_migrations(&pool, &[set]).await.unwrap();
    run_migrations(&pool, &[set]).await.unwrap();
    let (n,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let (applied,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM public.schema_migrations WHERE module=$1")
            .bind(module)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(applied, 1);

    let migs2: &'static [Migration] = Box::leak(Box::new([Migration {
        version: 1,
        name: "init",
        sql: "SELECT 1",
    }]));
    let err = run_migrations(
        &pool,
        &[ModuleMigrations {
            module,
            migrations: migs2,
        }],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::ChecksumMismatch { version: 1, .. }));

    sqlx::query(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM public.schema_migrations WHERE module=$1")
        .bind(module)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn outbox_roundtrip() {
    let Some(pool) = pool().await else { return };
    run_migrations(&pool, &[OUTBOX_MIGRATIONS]).await.unwrap();
    let stream = format!("test.{}", uuid::Uuid::new_v4());

    let mut tx = pool.begin().await.unwrap();
    let a = enqueue_outbox(&mut tx, &stream, &serde_json::json!({"n": 1}))
        .await
        .unwrap();
    let b = enqueue_outbox(&mut tx, &stream, &serde_json::json!({"n": 2}))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Other tests/runs may leave rows; claim generously and filter by our stream.
    let rows: Vec<_> = claim_outbox_batch(&pool, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.stream == stream)
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, a);
    assert_eq!(rows[1].id, b);
    assert_eq!(rows[0].attempts, 1);

    mark_failed(&pool, b, "boom").await.unwrap();
    mark_published(&pool, &[a]).await.unwrap();
    let rows: Vec<_> = claim_outbox_batch(&pool, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.stream == stream)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, b);
    assert_eq!(rows[0].last_error.as_deref(), Some("boom"));
    assert_eq!(rows[0].attempts, 2);

    mark_published(&pool, &[b]).await.unwrap();
    sqlx::query("DELETE FROM public.outbox WHERE stream=$1")
        .bind(&stream)
        .execute(&pool)
        .await
        .unwrap();
}
