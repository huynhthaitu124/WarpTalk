//! Room ownership and lifecycle over Redis (ADR-2): one replica owns a room through the lease
//! `pipeline:room:{id}:owner`, renewed every ttl/3 and released on end. Rooms are discovered
//! from `RoomStarted` commands and by periodically scanning audio-routes snapshots (so a
//! replica that died is replaced by another one within one scan interval after lease expiry).
//! Routes updates (`AUDIO_ROUTES_UPDATED` on `translationRoom:{id}:events`) are forwarded to
//! the room worker, which adds/removes speakers; `RoomEnded`, `CancelRoom` and
//! `TranslationStopped` shut the room down.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use redis::aio::ConnectionManager;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use warptalk_bus::{BusError, release_lease, renew_lease, subscribe, try_acquire_lease};
use warptalk_contracts::channels::{self, TRANSLATION_ROOM_COMMANDS};
use warptalk_contracts::events::{AudioRoutesSnapshot, RoomCommand};
use warptalk_contracts::keys;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::{PipelineConfig, SpeechToText, VadFactory};

use crate::downstream::DownstreamStage;
use crate::results::ResultSink;
use crate::routes::RoutesView;
use crate::transport::MediaTransport;
use crate::worker::{RoomWorker, RoomWorkerDeps, WorkerError};

pub const ROOMS_ACTIVE_METRIC: &str = "warptalk_pipeline_rooms_active";
pub const LEASE_LOST_METRIC: &str = "warptalk_pipeline_lease_lost_total";
/// Event type carried on `translationRoom:{id}:events` (spec 10 section 3.7).
pub const AUDIO_ROUTES_UPDATED: &str = "AUDIO_ROUTES_UPDATED";
pub const DEFAULT_SCAN_PATTERN: &str = "translationRoom:*:audio_routes";

#[derive(Debug, Error)]
pub enum HostError {
    #[error("bus: {0}")]
    Bus(#[from] BusError),
    #[error("redis: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("room worker: {0}")]
    Worker(#[from] WorkerError),
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Lease owner id; unique per replica.
    pub host_id: String,
    pub lease_ttl: Duration,
    /// Period of the routes-snapshot scan; `None` disables it (commands only).
    pub scan_interval: Option<Duration>,
    /// Pub/sub channel of room commands.
    pub commands_channel: String,
    /// SCAN MATCH pattern of routes snapshot keys; `None` disables the scan.
    pub scan_pattern: Option<String>,
    /// Identity the agent joins rooms under.
    pub agent_identity: String,
}

impl HostConfig {
    pub fn new(host_id: impl Into<String>, lease_ttl: Duration) -> Self {
        let host_id = host_id.into();
        Self {
            agent_identity: format!("pipeline-agent-{host_id}"),
            host_id,
            lease_ttl,
            scan_interval: Some(Duration::from_secs(5)),
            commands_channel: TRANSLATION_ROOM_COMMANDS.to_owned(),
            scan_pattern: Some(DEFAULT_SCAN_PATTERN.to_owned()),
        }
    }
}

pub struct HostDeps {
    pub redis: ConnectionManager,
    pub redis_url: String,
    pub transport: Arc<dyn MediaTransport>,
    pub stt: Arc<dyn SpeechToText>,
    pub vad_factory: VadFactory,
    pub downstream: Arc<dyn DownstreamStage>,
    pub results: Arc<dyn ResultSink>,
    /// Hot-reloadable pipeline config shared by all rooms.
    pub config: watch::Receiver<PipelineConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    AlreadyRunning,
    /// Another replica holds the lease.
    NotOwner,
    /// The room is over (terminal status in its snapshot); nothing started.
    Inactive,
}

type EventStream = Pin<Box<dyn Stream<Item = (String, String)> + Send>>;

struct RoomEntry {
    worker: Arc<RoomWorker>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    rooms: HashMap<RoomId, RoomEntry>,
    starting: HashSet<RoomId>,
}

struct Inner {
    cfg: HostConfig,
    deps: HostDeps,
    state: Mutex<State>,
}

/// Cheap to clone.
#[derive(Clone)]
pub struct PipelineHost {
    inner: Arc<Inner>,
}

#[derive(Deserialize)]
struct RoomEventMsg {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: serde_json::Value,
}

impl PipelineHost {
    pub fn new(cfg: HostConfig, deps: HostDeps) -> Self {
        Self {
            inner: Arc::new(Inner {
                cfg,
                deps,
                state: Mutex::new(State::default()),
            }),
        }
    }

    pub fn host_id(&self) -> &str {
        &self.inner.cfg.host_id
    }

    pub fn active_rooms(&self) -> Vec<RoomId> {
        self.inner.lock().rooms.keys().copied().collect()
    }

    pub fn room_worker(&self, room: RoomId) -> Option<Arc<RoomWorker>> {
        self.inner.lock().rooms.get(&room).map(|e| e.worker.clone())
    }

    /// Takes ownership of `room` if nobody else owns it and starts its pipeline.
    pub async fn ensure_room(&self, room: RoomId) -> Result<StartOutcome, HostError> {
        {
            let mut st = self.inner.lock();
            if st.rooms.contains_key(&room) || !st.starting.insert(room) {
                return Ok(StartOutcome::AlreadyRunning);
            }
        }
        let res = self.inner.clone().start(room).await;
        self.inner.lock().starting.remove(&room);
        self.inner.update_gauge();
        res
    }

    /// Shuts the room's pipeline down (drains speakers) and releases the lease.
    pub async fn stop_room(&self, room: RoomId) {
        let entry = self.inner.lock().rooms.remove(&room);
        if let Some(e) = entry {
            let _ = e.stop.send(true);
            let _ = e.task.await;
        }
        self.inner.update_gauge();
    }

    /// Stops every room concurrently.
    pub async fn shutdown_all(&self) {
        let entries: Vec<RoomEntry> = self.inner.lock().rooms.drain().map(|(_, e)| e).collect();
        for e in &entries {
            let _ = e.stop.send(true);
        }
        for e in entries {
            let _ = e.task.await;
        }
        self.inner.update_gauge();
    }

    /// Command listener + periodic scan, until `shutdown` turns true; then stops all rooms.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let inner = &self.inner;
        'outer: loop {
            if *shutdown.borrow() {
                break;
            }
            let channels = [inner.cfg.commands_channel.clone()];
            let mut cmds: EventStream = match subscribe(&inner.deps.redis_url, &channels).await {
                Ok(s) => Box::pin(s),
                Err(e) => {
                    warn!(error = %e, "subscribing to room commands failed, retrying");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                        _ = wait_true(&mut shutdown) => break 'outer,
                    }
                }
            };
            info!(host = %inner.cfg.host_id, channel = %inner.cfg.commands_channel, "listening for room commands");
            let mut tick = inner.cfg.scan_interval.map(|d| {
                let mut t = tokio::time::interval(d);
                t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                t
            });
            loop {
                tokio::select! {
                    biased;
                    _ = wait_true(&mut shutdown) => break 'outer,
                    msg = cmds.next() => match msg {
                        Some((_, payload)) => self.handle_command(&payload),
                        None => { warn!("room command subscription ended, resubscribing"); break; }
                    },
                    _ = tick_or_pending(&mut tick) => {
                        if let Err(e) = self.scan().await {
                            warn!(error = %e, "routes snapshot scan failed");
                        }
                    }
                }
            }
        }
        self.shutdown_all().await;
    }

    fn handle_command(&self, payload: &str) {
        let Ok(cmd) = serde_json::from_str::<RoomCommand>(payload) else {
            debug!("ignoring undecodable room command");
            return;
        };
        let (room, start) = match cmd {
            RoomCommand::RoomStarted { room_id, .. } => (room_id, true),
            RoomCommand::RoomEnded { room_id }
            | RoomCommand::CancelRoom { room_id }
            | RoomCommand::TranslationStopped { room_id } => (room_id, false),
            _ => return,
        };
        let host = self.clone();
        tokio::spawn(async move {
            if start {
                match host.ensure_room(room).await {
                    Ok(o) => debug!(%room, outcome = ?o, "RoomStarted handled"),
                    Err(e) => warn!(%room, error = %e, "starting room failed"),
                }
            } else {
                host.stop_room(room).await;
            }
        });
    }

    /// Looks at the routes snapshots and starts rooms that have translation active.
    async fn scan(&self) -> Result<(), HostError> {
        let Some(pattern) = self.inner.cfg.scan_pattern.clone() else {
            return Ok(());
        };
        let mut conn = self.inner.deps.redis.clone();
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(200)
                .query_async(&mut conn)
                .await?;
            for key in keys {
                let Some(room) = room_from_routes_key(&key) else {
                    continue;
                };
                if self.inner.lock().rooms.contains_key(&room) {
                    continue;
                }
                match self.inner.read_view(room).await {
                    Ok(Some(v)) if v.translation_active && !v.ended => {
                        let host = self.clone();
                        tokio::spawn(async move {
                            if let Err(e) = host.ensure_room(room).await {
                                warn!(%room, error = %e, "starting scanned room failed");
                            }
                        });
                    }
                    Ok(_) => {}
                    Err(e) => warn!(%room, error = %e, "reading routes snapshot failed"),
                }
            }
            cursor = next;
            if cursor == 0 {
                return Ok(());
            }
        }
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update_gauge(&self) {
        let n = self.lock().rooms.len();
        metrics::gauge!(ROOMS_ACTIVE_METRIC).set(n as f64);
    }

    fn lease_ms(&self) -> u64 {
        self.cfg.lease_ttl.as_millis() as u64
    }

    async fn start(self: Arc<Self>, room: RoomId) -> Result<StartOutcome, HostError> {
        let key = keys::pipeline_room_owner(room);
        let mut conn = self.deps.redis.clone();
        if !try_acquire_lease(&mut conn, &key, &self.cfg.host_id, self.lease_ms()).await? {
            return Ok(StartOutcome::NotOwner);
        }
        match self.clone().start_owned(room).await {
            Ok(StartOutcome::Started) => Ok(StartOutcome::Started),
            other => {
                let _ = release_lease(&mut conn, &key, &self.cfg.host_id).await;
                other
            }
        }
    }

    /// Lease already held.
    async fn start_owned(self: Arc<Self>, room: RoomId) -> Result<StartOutcome, HostError> {
        // Subscribe before reading the snapshot so no update falls in between.
        let events: EventStream = Box::pin(
            subscribe(&self.deps.redis_url, &[channels::room_events(room)]).await?,
        );
        let view = self.read_view(room).await?.unwrap_or_default();
        if view.ended {
            return Ok(StartOutcome::Inactive);
        }
        let flash_override = self.read_flash_override(room).await;
        let worker = Arc::new(
            RoomWorker::start(RoomWorkerDeps {
                room,
                transport: self.deps.transport.clone(),
                stt: self.deps.stt.clone(),
                vad_factory: self.deps.vad_factory.clone(),
                downstream: self.deps.downstream.clone(),
                results: self.deps.results.clone(),
                config: self.deps.config.clone(),
                flash_override,
                identity: self.cfg.agent_identity.clone(),
                initial_routes: view,
            })
            .await?,
        );
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(self.clone().supervise(room, worker.clone(), stop_rx, events));
        self.lock().rooms.insert(
            room,
            RoomEntry {
                worker,
                stop: stop_tx,
                task,
            },
        );
        info!(%room, host = %self.cfg.host_id, "room pipeline started");
        Ok(StartOutcome::Started)
    }

    /// Renews the lease, forwards routes updates, ends the room on terminal events.
    async fn supervise(
        self: Arc<Self>,
        room: RoomId,
        worker: Arc<RoomWorker>,
        mut stop: watch::Receiver<bool>,
        events: EventStream,
    ) {
        let key = keys::pipeline_room_owner(room);
        let mut conn = self.deps.redis.clone();
        let mut renew = tokio::time::interval(self.cfg.lease_ttl / 3);
        renew.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        renew.tick().await; // first tick is immediate
        let mut last_ok = Instant::now();
        let mut events = Some(events);
        let reason = loop {
            tokio::select! {
                biased;
                _ = wait_true(&mut stop) => break "stopped",
                _ = renew.tick() => {
                    match renew_lease(&mut conn, &key, &self.cfg.host_id, self.lease_ms()).await {
                        Ok(true) => last_ok = Instant::now(),
                        Ok(false) => {
                            metrics::counter!(LEASE_LOST_METRIC).increment(1);
                            warn!(%room, "lease lost, shutting the room down");
                            break "lease_lost";
                        }
                        Err(e) => {
                            warn!(%room, error = %e, "lease renewal failed");
                            if last_ok.elapsed() > self.cfg.lease_ttl {
                                metrics::counter!(LEASE_LOST_METRIC).increment(1);
                                break "lease_expired";
                            }
                        }
                    }
                    if events.is_none() {
                        // The pub/sub connection dropped; resubscribe and catch up from the key.
                        if let Ok(s) = subscribe(&self.deps.redis_url, &[channels::room_events(room)]).await {
                            events = Some(Box::pin(s));
                            if let Ok(Some(v)) = self.read_view(room).await {
                                if v.ended { break "room_ended"; }
                                worker.set_routes(v);
                            }
                        }
                    }
                }
                msg = next_event(&mut events) => match msg {
                    Some((_, payload)) => {
                        if let Some(v) = self.view_from_event(room, &payload).await {
                            if v.ended { break "room_ended"; }
                            worker.set_routes(v);
                        }
                    }
                    None => { warn!(%room, "room events subscription ended"); events = None; }
                },
            }
        };
        info!(%room, reason, "stopping room pipeline");
        worker.stop().await;
        let _ = release_lease(&mut conn, &key, &self.cfg.host_id).await;
        {
            let mut st = self.lock();
            if st.rooms.get(&room).is_some_and(|e| Arc::ptr_eq(&e.worker, &worker)) {
                st.rooms.remove(&room);
            }
        }
        self.update_gauge();
    }

    async fn read_view(&self, room: RoomId) -> Result<Option<RoutesView>, HostError> {
        let mut conn = self.deps.redis.clone();
        let raw: Option<String> = redis::cmd("GET")
            .arg(keys::audio_routes(room))
            .query_async(&mut conn)
            .await?;
        Ok(raw.and_then(|r| match serde_json::from_str::<AudioRoutesSnapshot>(&r) {
            Ok(s) => Some(RoutesView::from_snapshot(&s)),
            Err(e) => {
                warn!(%room, error = %e, "undecodable routes snapshot");
                None
            }
        }))
    }

    async fn read_flash_override(&self, room: RoomId) -> Option<bool> {
        let mut conn = self.deps.redis.clone();
        let raw: Option<String> = redis::cmd("GET")
            .arg(keys::flash_mode(room))
            .query_async(&mut conn)
            .await
            .ok()?;
        match raw?.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" => Some(false),
            _ => None,
        }
    }

    /// The snapshot carried by an `AUDIO_ROUTES_UPDATED` event, or the one in Redis when the
    /// event carries none.
    async fn view_from_event(&self, room: RoomId, payload: &str) -> Option<RoutesView> {
        let msg: RoomEventMsg = serde_json::from_str(payload).ok()?;
        if msg.kind != AUDIO_ROUTES_UPDATED {
            return None;
        }
        let snapshot = match msg.data {
            serde_json::Value::String(s) => serde_json::from_str::<AudioRoutesSnapshot>(&s).ok(),
            v @ serde_json::Value::Object(_) => serde_json::from_value(v).ok(),
            _ => None,
        };
        match snapshot {
            Some(s) => Some(RoutesView::from_snapshot(&s)),
            None => self.read_view(room).await.ok().flatten(),
        }
    }
}

/// `translationRoom:{uuid}:audio_routes` -> room id.
fn room_from_routes_key(key: &str) -> Option<RoomId> {
    key.strip_prefix("translationRoom:")?
        .strip_suffix(":audio_routes")?
        .parse()
        .ok()
}

async fn wait_true(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            // Sender dropped: treat as shutdown.
            return;
        }
    }
}

async fn next_event(events: &mut Option<EventStream>) -> Option<(String, String)> {
    match events {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

async fn tick_or_pending(tick: &mut Option<tokio::time::Interval>) {
    match tick {
        Some(t) => {
            t.tick().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn routes_key_roundtrip() {
        let r = RoomId::new();
        assert_eq!(room_from_routes_key(&keys::audio_routes(r)), Some(r));
        assert_eq!(room_from_routes_key("translationRoom:x:audio_routes"), None);
        assert_eq!(room_from_routes_key("other"), None);
    }
}
