#![allow(clippy::unwrap_used)]
//! Post-session summary through the worker: real `run_consumer` on `stt:results`
//! (group `meeting-summary`), the worker's Postgres adapters over the transcript and room
//! tables, a fake LLM. Skips without DATABASE_URL / REDIS_URL.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use redis::streams::StreamRangeReply;
use serde_json::json;
use tokio::sync::watch;
use uuid::Uuid;
use warptalk_assistant::suggestion::{
    DataBoundary, LlmResolver, Purpose, ResolveError, ResolvedLlm,
};
use warptalk_assistant::summary::NS_SUMMARY;
use warptalk_contracts::events::SttResult;
use warptalk_contracts::streams::{ASSISTANT_SUMMARY_RESULTS, STT_RESULTS};
use warptalk_kernel::{RoomId, UserId, WorkspaceId};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, Usage,
};
use warptalk_settings::{Scope, SettingsStore, register_defaults};
use warptalk_worker::{build_summary, join_consumers, spawn_consumers};

#[derive(Default)]
struct FakeLlm {
    calls: Mutex<usize>,
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _: &str) -> Capabilities {
        Capabilities::conservative()
    }
    async fn complete(&self, _: &Request) -> Result<Response, LlmError> {
        *self.calls.lock().unwrap() += 1;
        let out = json!({
            "summary_short": "Short.", "summary_detailed": "Detailed.",
            "decisions": ["Ship"], "open_questions": [], "topics": ["release"],
            "action_items": [
                {"task": "Send the release notes", "owner": "Bob", "due": "by Friday", "source_segments": ["s1"]},
                {"task": "Call legal", "owner": "Nobody Named", "due": null, "source_segments": []}
            ]
        });
        Ok(Response {
            parts: vec![Part::Text(out.to_string())],
            stop_reason: StopReason::End,
            usage: Usage {
                input_uncached: 10,
                output: 5,
                ..Default::default()
            },
            provider_request_id: None,
        })
    }
    async fn stream(
        &self,
        _: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest("no streaming".into()))
    }
    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Resolver(Arc<FakeLlm>);

#[async_trait]
impl LlmResolver for Resolver {
    async fn for_purpose(&self, _: WorkspaceId, _: Purpose) -> Result<ResolvedLlm, ResolveError> {
        Ok(ResolvedLlm {
            provider: self.0.clone(),
            model: None,
            boundary: DataBoundary::External,
            key_source: Default::default(),
        })
    }
}

async fn seed(pool: &sqlx::PgPool, ws: Uuid, room: Uuid) {
    sqlx::query(
        "INSERT INTO workspace.workspaces (id, name, slug, owner_id, settings, created_at) \
         VALUES ($1, 'ws', $2, $3, '{\"externalLlmAllowed\": true}'::jsonb, now())",
    )
    .bind(ws)
    .bind(format!("ws-{ws}"))
    .bind(Uuid::now_v7())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO translation_room.translation_rooms \
         (id, workspace_id, host_id, title, translation_room_code, source_language, \
          target_languages, created_at) \
         VALUES ($1, $2, $3, 'Release sync', $4, 'en', '[\"vi\"]'::jsonb, now())",
    )
    .bind(room)
    .bind(ws)
    .bind(Uuid::now_v7())
    .bind(room.simple().to_string()[..12].to_owned())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO translation_room.translation_room_participants \
         (id, translation_room_id, display_name, listen_language, speak_language, created_at, updated_at) \
         VALUES ($1, $2, 'Bob', 'en', 'en', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(room)
    .execute(pool)
    .await
    .unwrap();
    let transcript = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO transcript.transcripts \
         (id, workspace_id, translation_room_id, source_language, status, created_at) \
         VALUES ($1, $2, $3, 'en', 'FINALIZED', now())",
    )
    .bind(transcript)
    .bind(ws)
    .bind(room)
    .execute(pool)
    .await
    .unwrap();
    for (i, (who, text)) in [
        ("Ann", "Welcome everyone."),
        ("Bob", "I will send the release notes by Friday."),
        ("Ann", "Great, thanks."),
    ]
    .iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO transcript.transcript_segments \
             (id, transcript_id, speaker_name, original_text, original_language, start_time_ms, \
              end_time_ms, sequence_order, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'en', $5, $6, $7, now(), now())",
        )
        .bind(Uuid::now_v7())
        .bind(transcript)
        .bind(*who)
        .bind(*text)
        .bind((i * 1000) as i32)
        .bind((i * 1000 + 900) as i32)
        .bind(i as i32 + 1)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn statuses(pool: &sqlx::PgPool, room: Uuid) -> Vec<(i32, String)> {
    sqlx::query_as(
        "SELECT version, status FROM assistant.meeting_summaries WHERE room_id = $1 ORDER BY version",
    )
    .bind(room)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn wait_for(pool: &sqlx::PgPool, room: Uuid, want: &[(i32, &str)]) {
    for _ in 0..100 {
        let got = statuses(pool, room).await;
        if got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g.0 == w.0 && g.1 == w.1) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "timed out; have {:?}, want {want:?}",
        statuses(pool, room).await
    );
}

#[tokio::test]
async fn end_marker_on_the_stream_produces_exactly_one_summary_and_regenerate_runs_via_the_stream()
{
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return;
    };
    let pool = warptalk_db::connect(&db, 6).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_settings::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
            warptalk_room::MIGRATIONS,
            warptalk_transcript::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let settings = SettingsStore::new(pool.clone(), Some(redis.clone()));
    register_defaults(&settings);
    let (ws, room) = (Uuid::now_v7(), Uuid::now_v7());
    seed(&pool, ws, room).await;
    let llm = Arc::new(FakeLlm::default());
    let summary = build_summary(
        pool.clone(),
        &redis_url,
        Arc::new(Resolver(llm.clone())),
        &settings,
    )
    .await
    .unwrap();
    settings
        .put(
            Scope::Workspace(WorkspaceId::from_uuid(ws)),
            NS_SUMMARY,
            json!({"backoff_base_ms": 0, "transcript_wait_secs": 0, "transcript_poll_ms": 10}),
            None,
        )
        .await
        .unwrap();

    let (tx, rx) = watch::channel(false);
    let handles = spawn_consumers(&redis_url, summary.consumers(), &rx)
        .await
        .unwrap();
    // run_consumer creates its groups at `$`: give it time before the first message.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Ordinary segments and a duplicate marker: still exactly one job.
    let mut conn = redis.clone();
    let room_id = RoomId::from_uuid(room);
    let mut seg = SttResult::meeting_end("seg-1".into(), room_id, 1);
    seg.text = "hello".into();
    warptalk_bus::xadd_json(&mut conn, STT_RESULTS, &seg, 10_000)
        .await
        .unwrap();
    for i in 0..2 {
        let m = SttResult::meeting_end(format!("end-{i}"), room_id, 10 + i);
        warptalk_bus::xadd_json(&mut conn, STT_RESULTS, &m, 10_000)
            .await
            .unwrap();
    }
    wait_for(&pool, room, &[(1, "completed")]).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        statuses(&pool, room).await,
        vec![(1, "completed".to_owned())]
    );
    assert_eq!(*llm.calls.lock().unwrap(), 1);

    let items: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT task, owner, due FROM assistant.action_items WHERE room_id = $1 ORDER BY created_at, id",
    )
    .bind(room)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0],
        (
            "Send the release notes".into(),
            Some("Bob".into()),
            Some("by Friday".into())
        )
    );
    assert_eq!(
        items[1],
        ("Call legal".into(), None, None),
        "owner not in the transcript is dropped"
    );
    let content: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM assistant.meeting_summaries WHERE room_id = $1")
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        content["actionItems"][0]["sourceSegmentIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(content["meta"]["totalSegments"], 3);

    // A result event was published.
    let events: StreamRangeReply = redis::cmd("XREVRANGE")
        .arg(ASSISTANT_SUMMARY_RESULTS)
        .arg("+")
        .arg("-")
        .arg("COUNT")
        .arg(50)
        .query_async(&mut conn)
        .await
        .unwrap();
    let mine = events
        .ids
        .iter()
        .filter_map(|e| e.get::<String>("data"))
        .filter(|d| d.contains(&room.to_string()))
        .count();
    assert_eq!(mine, 1);

    // Regenerate publishes on assistant:summary_requests; the consumer runs it (no sweeper).
    let job = summary
        .service()
        .regenerate(WorkspaceId::from_uuid(ws), room_id, UserId::new())
        .await
        .unwrap();
    assert_eq!(job.version, 2);
    wait_for(&pool, room, &[(1, "completed"), (2, "completed")]).await;
    assert_eq!(*llm.calls.lock().unwrap(), 2);

    let _ = tx.send(true);
    join_consumers(handles).await;
}
