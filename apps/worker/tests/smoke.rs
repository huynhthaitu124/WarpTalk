#![allow(clippy::unwrap_used)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;
use warptalk_kernel::{RoomId, WorkspaceId};
use warptalk_meeting::application::ports::ChatTargetLanguages;
use warptalk_transcript::application::ports::{RoomDirectory, WorkspaceAiPolicy};
use warptalk_worker::adapters::{PgChatTargetLanguages, PgRoomDirectory, PgWorkspaceAiPolicy};

fn env() -> Option<(String, String)> {
    match (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL")) {
        (Ok(d), Ok(r)) => Some((d, r)),
        _ => {
            println!("DATABASE_URL / REDIS_URL unset; skipping");
            None
        }
    }
}

/// Run the binary for ~3 s against the local services, SIGTERM it, expect a clean exit.
#[tokio::test]
async fn binary_runs_and_exits_cleanly_on_sigterm() {
    let Some((db, redis)) = env() else { return };
    let pool = warptalk_db::connect(&db, 2).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_transcript::MIGRATIONS,
            warptalk_meeting::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    pool.close().await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_warptalk-worker"))
        .env("DATABASE_URL", db)
        .env("REDIS_URL", redis)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "worker exited before SIGTERM"
    );
    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(killed.success());
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "worker did not stop after SIGTERM"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(status.success(), "unclean exit: {status:?}");
}

#[tokio::test]
async fn adapters_read_room_workspace_and_ai_settings() {
    let Some((db, _)) = env() else { return };
    let pool = warptalk_db::connect(&db, 2).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[warptalk_workspace::MIGRATIONS, warptalk_room::MIGRATIONS],
    )
    .await
    .unwrap();

    let ws = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO workspace.workspaces (id, name, slug, owner_id, settings, created_at) \
         VALUES ($1, 'w', $2, $3, '{\"aiRetrievalAllowed\": true}'::jsonb, now())",
    )
    .bind(ws)
    .bind(format!("w-{ws}"))
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    let policy = PgWorkspaceAiPolicy::new(pool.clone());
    let ws_id = WorkspaceId::from_uuid(ws);
    assert!(policy.ai_retrieval_allowed(ws_id).await.unwrap());
    assert!(!policy.external_llm_allowed(ws_id).await.unwrap());
    // Unknown workspace defaults to false.
    assert!(
        !policy
            .ai_retrieval_allowed(WorkspaceId::new())
            .await
            .unwrap()
    );

    let room = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO translation_room.translation_rooms \
         (id, workspace_id, host_id, title, translation_room_code, source_language, created_at) \
         VALUES ($1, $2, $3, 't', $4, 'vi', now())",
    )
    .bind(room)
    .bind(ws)
    .bind(Uuid::now_v7())
    .bind(room.simple().to_string()[..12].to_owned())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE translation_room.translation_rooms \
         SET target_languages = '[\"en\", \"vi\", \"ja\"]'::jsonb WHERE id = $1",
    )
    .bind(room)
    .execute(&pool)
    .await
    .unwrap();
    let targets = PgChatTargetLanguages::new(pool.clone());
    assert_eq!(
        targets.for_room(RoomId::from_uuid(room)).await.unwrap(),
        vec!["vi", "en", "ja"]
    );
    assert!(targets.for_room(RoomId::new()).await.unwrap().is_empty());
    let dir = PgRoomDirectory::new(pool);
    let found = dir.room(RoomId::from_uuid(room)).await.unwrap().unwrap();
    assert_eq!(found.workspace_id, ws_id);
    assert!(dir.room(RoomId::new()).await.unwrap().is_none());
}
