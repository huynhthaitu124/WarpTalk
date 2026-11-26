#![allow(clippy::unwrap_used)]
//! `usage:events` entries are metered once, malformed ones are dead-lettered.

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;
use warptalk_bus::{HandlerError, StreamMessage};

fn msg(data: String) -> StreamMessage {
    StreamMessage {
        stream: "usage:events".into(),
        id: "1-0".into(),
        fields: HashMap::from([("data".to_owned(), data)]),
    }
}

#[tokio::test]
async fn usage_entry_is_debited_once_and_garbage_is_fatal() {
    let (Ok(db), Ok(redis)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL")) else {
        eprintln!("DATABASE_URL/REDIS_URL not set; skipping");
        return;
    };
    let pool = warptalk_db::connect(&db, 3).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_billing::MIGRATIONS])
        .await
        .unwrap();
    let (_, handler) = warptalk_worker::metering::metering_consumer(pool.clone(), &redis)
        .await
        .unwrap();
    let ws = Uuid::now_v7();
    let ev = json!({
        "event_id": format!("worker-test:{ws}"), "workspace_id": ws, "feature": "stt",
        "model": "m", "audio_ms": 10_000, "boundary": "platform",
        "occurred_at": chrono_now(),
    })
    .to_string();
    handler.handle(&msg(ev.clone())).await.unwrap();
    handler.handle(&msg(ev)).await.unwrap(); // redelivery
    let (records, balance): (i64, i32) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM subscription.usage_records WHERE workspace_id = $1), \
                (SELECT credits_remaining FROM subscription.subscriptions WHERE workspace_id = $1)",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(records, 1);
    assert_eq!(balance, 1800 - 10);
    let bad = handler.handle(&msg("{nope".into())).await.unwrap_err();
    assert!(matches!(bad, HandlerError::Fatal(_)));
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}
