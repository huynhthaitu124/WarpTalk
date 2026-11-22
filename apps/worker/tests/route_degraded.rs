//! `translationRoom:system_events` `ROUTE_DEGRADED` (producer: the pipeline's TTS stage event,
//! encoded as the pipeline's result sink does) updates the route status in the room module.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use tokio::sync::watch;
use uuid::Uuid;
use warptalk_contracts::events::TranslationRoomSystemEvent;
use warptalk_tts::RouteTier;
use warptalk_worker::route_status::route_status_consumer;
use warptalk_worker::{join_consumers, spawn_consumers};

/// The payload the pipeline writes in `handle_tts_event` (`from`/`to` are `{tier:?}`).
fn degraded_event(
    room: Uuid,
    route: Option<Uuid>,
    from: RouteTier,
    to: RouteTier,
) -> TranslationRoomSystemEvent {
    TranslationRoomSystemEvent {
        event_type: "ROUTE_DEGRADED".into(),
        route_id: route,
        room_id: room.into(),
        payload: serde_json::json!({
            "speaker_id": Uuid::now_v7().to_string(),
            "target_lang": "en",
            "from": format!("{from:?}"),
            "to": format!("{to:?}"),
            "reason": "clone_failed",
        }),
    }
}

async fn route_status(pool: &sqlx::PgPool, route: Uuid) -> String {
    sqlx::query_scalar(
        "SELECT status FROM translation_room.translation_room_audio_routes WHERE id = $1",
    )
    .bind(route)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn route_degraded_updates_route_status() {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return;
    };
    let pool = warptalk_db::connect(&db, 3).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_room::MIGRATIONS])
        .await
        .unwrap();

    let room = Uuid::now_v7();
    let (r1, r2, other_room_route) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (id, room_id) in [(r1, room), (r2, room), (other_room_route, Uuid::now_v7())] {
        sqlx::query(
            "INSERT INTO translation_room.translation_room_audio_routes \
             (id, translation_room_id, source_participant_id, target_participant_id, \
              source_language, target_language, status, is_current) \
             VALUES ($1, $2, $3, $4, 'vi', 'en', 'PENDING', true)",
        )
        .bind(id)
        .bind(room_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await
        .unwrap();
    }

    let (tx, rx) = watch::channel(false);
    let handles = spawn_consumers(&redis_url, vec![route_status_consumer(pool.clone())], &rx)
        .await
        .unwrap();
    // Let the consumer group exist before the entries are added.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut conn = warptalk_bus::connect(&redis_url).await.unwrap();
    let events = [
        degraded_event(room, Some(r1), RouteTier::Clone, RouteTier::TextOnly),
        degraded_event(room, Some(r2), RouteTier::Clone, RouteTier::Standard),
        // A route of another room is not touched, an event without a route id is acked.
        degraded_event(
            room,
            Some(other_room_route),
            RouteTier::Clone,
            RouteTier::TextOnly,
        ),
        degraded_event(room, None, RouteTier::Clone, RouteTier::TextOnly),
    ];
    for e in &events {
        warptalk_bus::xadd_json(&mut conn, "translationRoom:system_events", e, 10_000)
            .await
            .unwrap();
    }

    let started = Instant::now();
    loop {
        let (a, b) = (route_status(&pool, r1).await, route_status(&pool, r2).await);
        if a == "CAPTION_ONLY" && b == "STANDARD_VOICE" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "statuses not updated: {a} {b}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(route_status(&pool, other_room_route).await, "PENDING");

    let _ = tx.send(true);
    join_consumers(handles).await;
}
