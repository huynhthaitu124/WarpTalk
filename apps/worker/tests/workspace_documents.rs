//! The worker applies `embedding:index_results` to workspace documents.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use tokio::sync::watch;
use uuid::Uuid;
use warptalk_worker::{join_consumers, spawn_consumers, workspace_document_consumer};

#[tokio::test]
async fn index_results_update_document_status() {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return;
    };
    let pool = warptalk_db::connect(&db, 3).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_workspace::MIGRATIONS])
        .await
        .unwrap();

    let ws = Uuid::now_v7();
    let doc = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO workspace.workspaces (id, name, slug, owner_id, settings, created_at) \
         VALUES ($1, 'w', $2, $3, '{}', now())",
    )
    .bind(ws)
    .bind(format!("w-{}", ws.simple()))
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workspace.workspace_documents \
         (id, workspace_id, name, file_name, file_extension, mime_type, size_bytes, \
          storage_provider, storage_key, ingestion_status, ai_eligible, created_at) \
         VALUES ($1, $2, 'd', 'd.txt', 'txt', 'text/plain', 1, 'local', 'k', 'pending', false, now())",
    )
    .bind(doc)
    .bind(ws)
    .execute(&pool)
    .await
    .unwrap();

    let (tx, rx) = watch::channel(false);
    let handles = spawn_consumers(
        &redis_url,
        vec![workspace_document_consumer(pool.clone())],
        &rx,
    )
    .await
    .unwrap();
    // Let the consumer group exist before the entry is added.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut conn = warptalk_bus::connect(&redis_url).await.unwrap();
    warptalk_bus::xadd(
        &mut conn,
        "embedding:index_results",
        &[
            ("job_id", format!("{doc}:1")),
            ("workspace_id", ws.to_string()),
            ("collection_id", format!("workspace_{ws}")),
            ("source_type", "document".to_owned()),
            ("source_id", doc.to_string()),
            ("status", "indexed".to_owned()),
            ("chunks_indexed", "3".to_owned()),
            ("provider", "p".to_owned()),
            ("model", "m".to_owned()),
            ("dimensions", "8".to_owned()),
            ("reason", String::new()),
        ],
        10_000,
    )
    .await
    .unwrap();

    let started = Instant::now();
    let (status, eligible, indexed_at): (String, bool, Option<chrono::DateTime<chrono::Utc>>) = loop {
        let row: (String, bool, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
            "SELECT ingestion_status, ai_eligible, last_indexed_at \
             FROM workspace.workspace_documents WHERE id = $1",
        )
        .bind(doc)
        .fetch_one(&pool)
        .await
        .unwrap();
        if row.0 != "pending" || started.elapsed() > Duration::from_secs(10) {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let _ = tx.send(true);
    join_consumers(handles).await;
    assert_eq!(status, "completed");
    assert!(eligible);
    assert!(indexed_at.is_some());
}
