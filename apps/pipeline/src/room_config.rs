//! Per-room pipeline configuration (B-11). The web writes the `pipeline` settings namespace at
//! platform, workspace and room scope; the effective value of a room is the merge
//! `platform < workspace < room` (`SettingsStore` + [`ScopeChain::room`]). The room's workspace
//! comes from `translationRoom:{id}:workspace` (written by the api next to the routes snapshot).
//!
//! What follows the room scope: the VAD/STT knobs, `flash_mode_default` (the per-room flash
//! mode; the old `translationRoom:{id}:flash_mode` Redis key is gone) and the latency flags
//! that live in the runtime config. `mt`, `tts` and the remaining latency flags of the
//! downstream stage stay platform-wide (the stage is shared by every room of the process).

use async_trait::async_trait;
use tokio::sync::watch;
use tracing::{info, warn};
use uuid::Uuid;
use warptalk_kernel::{RoomId, WorkspaceId};
use warptalk_pipeline_runtime::PipelineConfig;
use warptalk_settings::{NS_PIPELINE, PipelineSettings, ScopeChain, SettingsStore};

use crate::config::pipeline_config;
use crate::tuning::PipelineExtras;

/// Gives a room its hot-reloading [`PipelineConfig`].
#[async_trait]
pub trait RoomConfigSource: Send + Sync {
    /// The config watch of `room`. `workspace` is `None` when the room's workspace is not known
    /// (only platform settings apply then). The watch ends when all receivers are dropped.
    async fn config_for(
        &self,
        room: RoomId,
        workspace: Option<WorkspaceId>,
    ) -> watch::Receiver<PipelineConfig>;
}

/// Every room follows one shared config (no database, tests).
pub struct SharedRoomConfig(pub watch::Receiver<PipelineConfig>);

#[async_trait]
impl RoomConfigSource for SharedRoomConfig {
    async fn config_for(
        &self,
        _: RoomId,
        _: Option<WorkspaceId>,
    ) -> watch::Receiver<PipelineConfig> {
        self.0.clone()
    }
}

/// Resolves the `pipeline` namespace through the settings store with the room's scope chain.
pub struct SettingsRoomConfig {
    store: SettingsStore,
    /// Platform-scope fallback (used when the store cannot be read).
    fallback: watch::Receiver<PipelineConfig>,
}

impl SettingsRoomConfig {
    pub fn new(store: SettingsStore, fallback: watch::Receiver<PipelineConfig>) -> Self {
        Self { store, fallback }
    }
}

#[async_trait]
impl RoomConfigSource for SettingsRoomConfig {
    async fn config_for(
        &self,
        room: RoomId,
        workspace: Option<WorkspaceId>,
    ) -> watch::Receiver<PipelineConfig> {
        let chain = match workspace {
            Some(ws) => ScopeChain::room(ws, room),
            None => {
                warn!(%room, "room workspace unknown: only platform pipeline settings apply");
                ScopeChain::platform()
            }
        };
        let settings = self
            .store
            .watch::<PipelineSettings>(NS_PIPELINE, chain)
            .await;
        let extras = self.store.watch::<PipelineExtras>(NS_PIPELINE, chain).await;
        let (mut settings, mut extras) = match (settings, extras) {
            (Ok(s), Ok(e)) => (s, e),
            (Err(e), _) | (_, Err(e)) => {
                warn!(%room, error = %e, "room pipeline settings unreadable: using platform config");
                return self.fallback.clone();
            }
        };
        let initial = pipeline_config(&settings.borrow(), &extras.borrow().latency);
        let initial = if initial.validate().is_ok() {
            initial
        } else {
            warn!(%room, "invalid room pipeline settings: using platform config");
            self.fallback.borrow().clone()
        };
        info!(%room, flash_mode = initial.flash_mode, "room pipeline config resolved");
        let (tx, rx) = watch::channel(initial);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tx.closed() => break,
                    r = settings.changed() => if r.is_err() { break },
                    r = extras.changed() => if r.is_err() { break },
                }
                let cfg = pipeline_config(
                    &settings.borrow_and_update(),
                    &extras.borrow_and_update().latency,
                );
                match cfg.validate() {
                    Ok(()) => {
                        tx.send_if_modified(|c| (*c != cfg).then(|| *c = cfg).is_some());
                    }
                    Err(e) => warn!(%room, error = %e, "ignoring invalid room pipeline settings"),
                }
            }
        });
        rx
    }
}

/// Parses the value of `translationRoom:{id}:workspace`.
pub fn parse_workspace(raw: &str) -> Option<WorkspaceId> {
    Uuid::parse_str(raw.trim()).ok().map(WorkspaceId::from_uuid)
}
