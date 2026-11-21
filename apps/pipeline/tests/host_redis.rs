#![allow(clippy::unwrap_used)]
//! Host tests against local Redis (skipped when REDIS_URL is unset).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use uuid::Uuid;
use warptalk_bus::publish_json;
use warptalk_contracts::events::{
    AudioRoute, AudioRoutesSnapshot, RoomCommand, RoomStartedState, RouteStatus,
};
use warptalk_contracts::{channels, keys};
use warptalk_kernel::{ParticipantId, RoomId};
use warptalk_pipeline::host::{HostConfig, HostDeps, PipelineHost, StartOutcome};
use warptalk_pipeline::{MemoryResultSink, SimulatedTransport, SttOnlyStage};
use warptalk_pipeline_runtime::PipelineConfig;
use warptalk_pipeline_runtime::testing::{FakeStt, energy_vad_factory};

struct Env {
    url: String,
    conn: redis::aio::ConnectionManager,
}

async fn env() -> Option<Env> {
    let Ok(url) = std::env::var("REDIS_URL") else {
        println!("REDIS_URL unset, skipping");
        return None;
    };
    let conn = warptalk_bus::connect(&url).await.unwrap();
    Some(Env { url, conn })
}

fn host(env: &Env, id: &str, channel: &str, scan: Option<String>) -> PipelineHost {
    let mut cfg = HostConfig::new(id, Duration::from_millis(900));
    cfg.commands_channel = channel.to_owned();
    cfg.scan_pattern = scan;
    cfg.scan_interval = cfg
        .scan_pattern
        .as_ref()
        .map(|_| Duration::from_millis(300));
    let (_tx, rx) = watch::channel(PipelineConfig::default());
    PipelineHost::new(
        cfg,
        HostDeps {
            redis: env.conn.clone(),
            redis_url: env.url.clone(),
            transport: Arc::new(SimulatedTransport::with_participants(vec![])),
            stt: Arc::new(FakeStt::default()),
            vad_factory: energy_vad_factory(),
            downstream: Arc::new(SttOnlyStage),
            results: Arc::new(MemoryResultSink::default()),
            config: rx,
        },
    )
}

fn snapshot(speakers: &[ParticipantId], active: bool, status: &str) -> AudioRoutesSnapshot {
    AudioRoutesSnapshot {
        generated_at: Utc::now(),
        room_status: status.into(),
        translation_active: active,
        room_languages: vec!["en".into(), "vi".into()],
        routes: speakers
            .iter()
            .map(|s| AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: *s,
                target_participant_id: ParticipantId::new(),
                source_language: "en".into(),
                target_language: "vi".into(),
                voice_clone_enabled: false,
                status: RouteStatus::Broadcasting,
            })
            .collect(),
    }
}

async fn put_snapshot(env: &Env, room: RoomId, s: &AudioRoutesSnapshot) {
    let mut c = env.conn.clone();
    let _: () = redis::cmd("SET")
        .arg(keys::audio_routes(room))
        .arg(serde_json::to_string(s).unwrap())
        .arg("EX")
        .arg(60)
        .query_async(&mut c)
        .await
        .unwrap();
}

async fn get(env: &Env, key: String) -> Option<String> {
    let mut c = env.conn.clone();
    redis::cmd("GET")
        .arg(key)
        .query_async(&mut c)
        .await
        .unwrap()
}

async fn wait(cond: impl AsyncFn() -> bool) {
    for _ in 0..100 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not reached");
}

fn started(room: RoomId) -> String {
    serde_json::to_string(&RoomCommand::RoomStarted {
        room_id: room,
        state: RoomStartedState {
            room_id: room,
            code: "abc".into(),
            status: "Active".into(),
            participants: vec![],
        },
    })
    .unwrap()
}

#[tokio::test]
async fn lease_prevents_two_owners_and_is_renewed() {
    let Some(env) = env().await else { return };
    let room = RoomId::new();
    let a = host(&env, "host-a", "test:unused", None);
    let b = host(&env, "host-b", "test:unused", None);
    let (ra, rb) = tokio::join!(a.ensure_room(room), b.ensure_room(room));
    let outcomes = [ra.unwrap(), rb.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == StartOutcome::Started)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == StartOutcome::NotOwner)
            .count(),
        1
    );
    let owner = if a.active_rooms().contains(&room) {
        "host-a"
    } else {
        "host-b"
    };
    assert_eq!(a.active_rooms().len() + b.active_rooms().len(), 1);
    // ttl is 900 ms: after 2.5 s the lease only survives through renewal.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        get(&env, keys::pipeline_room_owner(room)).await.as_deref(),
        Some(owner)
    );
    // Stopping releases it; the other host can then take over.
    let (own, other) = if owner == "host-a" {
        (&a, &b)
    } else {
        (&b, &a)
    };
    own.stop_room(room).await;
    assert_eq!(get(&env, keys::pipeline_room_owner(room)).await, None);
    assert_eq!(
        other.ensure_room(room).await.unwrap(),
        StartOutcome::Started
    );
    other.shutdown_all().await;
}

#[tokio::test]
async fn room_started_command_two_hosts_one_owner_then_room_ended() {
    let Some(env) = env().await else { return };
    let room = RoomId::new();
    let chan = format!("test:commands:{room}");
    let a = host(&env, "host-a", &chan, None);
    let b = host(&env, "host-b", &chan, None);
    let (sa, sb) = (watch::channel(false), watch::channel(false));
    let ta = tokio::spawn({
        let a = a.clone();
        let rx = sa.1.clone();
        async move { a.run(rx).await }
    });
    let tb = tokio::spawn({
        let b = b.clone();
        let rx = sb.1.clone();
        async move { b.run(rx).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await; // subscriptions up

    let mut c = env.conn.clone();
    let mut payload_conn = env.conn.clone();
    let _: i64 = redis::cmd("PUBLISH")
        .arg(&chan)
        .arg(started(room))
        .query_async(&mut payload_conn)
        .await
        .unwrap();
    wait(async || a.active_rooms().len() + b.active_rooms().len() >= 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        a.active_rooms().len() + b.active_rooms().len(),
        1,
        "exactly one owner"
    );

    publish_json(&mut c, &chan, &RoomCommand::RoomEnded { room_id: room })
        .await
        .unwrap();
    wait(async || a.active_rooms().is_empty() && b.active_rooms().is_empty()).await;
    wait(async || get(&env, keys::pipeline_room_owner(room)).await.is_none()).await;

    sa.0.send(true).unwrap();
    sb.0.send(true).unwrap();
    ta.await.unwrap();
    tb.await.unwrap();
}

#[tokio::test]
async fn routes_update_event_adds_speaker_and_ended_status_shuts_down() {
    let Some(env) = env().await else { return };
    let room = RoomId::new();
    let (p1, p2) = (ParticipantId::new(), ParticipantId::new());
    put_snapshot(&env, room, &snapshot(&[p1], true, "Active")).await;
    let h = host(&env, "host-a", "test:unused", None);
    assert_eq!(h.ensure_room(room).await.unwrap(), StartOutcome::Started);
    let worker = h.room_worker(room).unwrap();
    wait(async || worker.speakers() == vec![p1.to_string()]).await;

    let mut c = env.conn.clone();
    let ev =
        |s: &AudioRoutesSnapshot| serde_json::json!({"type": "AUDIO_ROUTES_UPDATED", "data": s});
    publish_json(
        &mut c,
        &channels::room_events(room),
        &ev(&snapshot(&[p1, p2], true, "Active")),
    )
    .await
    .unwrap();
    wait(async || worker.speakers().len() == 2).await;
    assert!(worker.routes().targets.contains("vi"));

    publish_json(
        &mut c,
        &channels::room_events(room),
        &ev(&snapshot(&[p1, p2], true, "Ended")),
    )
    .await
    .unwrap();
    wait(async || h.active_rooms().is_empty()).await;
    assert!(worker.is_finished());
    assert_eq!(get(&env, keys::pipeline_room_owner(room)).await, None);
}

#[tokio::test]
async fn scan_discovers_active_snapshot() {
    let Some(env) = env().await else { return };
    let room = RoomId::new();
    put_snapshot(
        &env,
        room,
        &snapshot(&[ParticipantId::new()], true, "Active"),
    )
    .await;
    let h = host(
        &env,
        "host-scan",
        "test:unused",
        Some(keys::audio_routes(room)),
    );
    let stop = watch::channel(false);
    let t = tokio::spawn({
        let h = h.clone();
        let rx = stop.1.clone();
        async move { h.run(rx).await }
    });
    wait(async || h.active_rooms() == vec![room]).await;
    stop.0.send(true).unwrap();
    t.await.unwrap();
    assert!(h.active_rooms().is_empty());
}
