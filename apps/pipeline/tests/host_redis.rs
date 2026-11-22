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
            room_config: None,
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
                source_user_id: None,
                target_user_id: None,
                source_display_name: None,
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

// ---- B-11: pipeline settings at workspace and room scope ----------------------------------------

type Seen = (
    RoomId,
    Option<warptalk_kernel::WorkspaceId>,
    watch::Receiver<PipelineConfig>,
);

struct RecordingConfig {
    inner: warptalk_pipeline::SettingsRoomConfig,
    seen: std::sync::Mutex<Vec<Seen>>,
}

#[async_trait::async_trait]
impl warptalk_pipeline::RoomConfigSource for RecordingConfig {
    async fn config_for(
        &self,
        room: RoomId,
        workspace: Option<warptalk_kernel::WorkspaceId>,
    ) -> watch::Receiver<PipelineConfig> {
        let rx = self.inner.config_for(room, workspace).await;
        self.seen
            .lock()
            .unwrap()
            .push((room, workspace, rx.clone()));
        rx
    }
}

/// The real host resolves the room's workspace from Redis and the real `SettingsStore` merges
/// platform < workspace < room: the room's `PipelineConfig` reflects the overrides and follows
/// later changes (including `flash_mode_default`, the per-room flash mode).
#[tokio::test]
async fn room_scope_pipeline_settings_apply_to_the_started_room() {
    use warptalk_kernel::WorkspaceId;
    use warptalk_settings::{NS_PIPELINE, Scope, SettingsStore, register_defaults};
    let Some(env) = env().await else { return };
    let Ok(db) = std::env::var("DATABASE_URL") else {
        println!("DATABASE_URL unset, skipping");
        return;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS])
        .await
        .unwrap();
    let store = SettingsStore::new(pool, Some(env.conn.clone()));
    register_defaults(&store);
    let (ws, room, other_room) = (WorkspaceId::new(), RoomId::new(), RoomId::new());
    store
        .put(
            Scope::Workspace(ws),
            NS_PIPELINE,
            serde_json::json!({"vad_silence_hangover_ms": 400}),
            None,
        )
        .await
        .unwrap();
    store
        .put(
            Scope::Room(room),
            NS_PIPELINE,
            serde_json::json!({"flash_mode_default": false}),
            None,
        )
        .await
        .unwrap();
    for r in [room, other_room] {
        let mut c = env.conn.clone();
        let _: () = redis::cmd("SET")
            .arg(format!("translationRoom:{r}:workspace"))
            .arg(ws.to_string())
            .arg("EX")
            .arg(60)
            .query_async(&mut c)
            .await
            .unwrap();
        put_snapshot(&env, r, &snapshot(&[ParticipantId::new()], true, "Active")).await;
    }

    let (_keep, platform) = watch::channel(PipelineConfig::default());
    let recording = Arc::new(RecordingConfig {
        inner: warptalk_pipeline::SettingsRoomConfig::new(store.clone(), platform.clone()),
        seen: Default::default(),
    });
    let mut cfg = HostConfig::new("host-cfg", Duration::from_millis(900));
    cfg.scan_pattern = None;
    cfg.scan_interval = None;
    let host = PipelineHost::new(
        cfg,
        HostDeps {
            redis: env.conn.clone(),
            redis_url: env.url.clone(),
            transport: Arc::new(SimulatedTransport::with_participants(vec![])),
            stt: Arc::new(FakeStt::default()),
            vad_factory: energy_vad_factory(),
            downstream: Arc::new(SttOnlyStage),
            results: Arc::new(MemoryResultSink::default()),
            config: platform,
            room_config: Some(recording.clone()),
        },
    );
    assert_eq!(host.ensure_room(room).await.unwrap(), StartOutcome::Started);
    assert_eq!(
        host.ensure_room(other_room).await.unwrap(),
        StartOutcome::Started
    );

    let seen = recording.seen.lock().unwrap().clone();
    let of = |r: RoomId| seen.iter().find(|(id, _, _)| *id == r).unwrap().clone();
    let (_, w, mut rx) = of(room);
    assert_eq!(
        w,
        Some(ws),
        "workspace read from translationRoom:{{id}}:workspace"
    );
    assert!(!rx.borrow().flash_mode, "room override");
    assert_eq!(
        rx.borrow().endpointer.vad_silence_hangover_ms,
        400,
        "workspace override"
    );
    let (_, _, rx_other) = of(other_room);
    assert!(rx_other.borrow().flash_mode, "other room: platform default");
    assert_eq!(rx_other.borrow().endpointer.vad_silence_hangover_ms, 400);

    // hot reload of the room override
    store
        .put(
            Scope::Room(room),
            NS_PIPELINE,
            serde_json::json!({"flash_mode_default": true}),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rx.borrow_and_update().flash_mode {
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("room config follows the settings change");
    host.shutdown_all().await;
}
