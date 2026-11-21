//! Per-room configuration resolution with hot reload.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::watch;
use warptalk_kernel::{RoomId, WorkspaceId};
use warptalk_settings::{NS_SUGGESTION, ScopeChain, SettingsStore};

use super::config::SuggestionConfig;

#[async_trait]
pub trait ConfigSource: Send + Sync {
    /// Effective configuration (platform > workspace > room) for a room, current as of now.
    async fn config_for(&self, workspace: WorkspaceId, room: RoomId) -> Arc<SuggestionConfig>;
    /// Release per-room resources when the room ends.
    fn forget(&self, room: RoomId);
}

/// Resolves through [`SettingsStore`]. Each room holds a `watch` receiver on its scope chain; the
/// store refreshes it on every relevant `put`/`delete` (and at least every TTL), so a change is
/// visible to the next segment without a restart. Invalid documents fall back to the last valid
/// value (store behaviour); a store failure on first use yields the defaults (`enabled = false`,
/// i.e. silence).
pub struct SettingsConfigSource {
    store: SettingsStore,
    watchers: Mutex<HashMap<RoomId, watch::Receiver<SuggestionConfig>>>,
}

impl SettingsConfigSource {
    pub fn new(store: SettingsStore) -> Self {
        Self {
            store,
            watchers: Mutex::new(HashMap::new()),
        }
    }

    fn cached(&self, room: RoomId) -> Option<Arc<SuggestionConfig>> {
        let map = self.watchers.lock().ok()?;
        map.get(&room).map(|rx| Arc::new(rx.borrow().clone()))
    }
}

#[async_trait]
impl ConfigSource for SettingsConfigSource {
    async fn config_for(&self, workspace: WorkspaceId, room: RoomId) -> Arc<SuggestionConfig> {
        if let Some(cfg) = self.cached(room) {
            return cfg;
        }
        match self
            .store
            .watch::<SuggestionConfig>(NS_SUGGESTION, ScopeChain::room(workspace, room))
            .await
        {
            Ok(rx) => {
                let cfg = Arc::new(rx.borrow().clone());
                if let Ok(mut map) = self.watchers.lock() {
                    map.insert(room, rx);
                }
                cfg
            }
            Err(e) => {
                tracing::warn!(%room, error = %e, "suggestion settings unavailable; staying silent");
                Arc::new(SuggestionConfig::default())
            }
        }
    }

    fn forget(&self, room: RoomId) {
        if let Ok(mut map) = self.watchers.lock() {
            map.remove(&room);
        }
    }
}

/// Fixed (but swappable) configuration, for tests and single-tenant setups.
pub struct StaticConfig(Mutex<Arc<SuggestionConfig>>);

impl StaticConfig {
    pub fn new(cfg: SuggestionConfig) -> Self {
        Self(Mutex::new(Arc::new(cfg)))
    }

    pub fn set(&self, cfg: SuggestionConfig) {
        if let Ok(mut g) = self.0.lock() {
            *g = Arc::new(cfg);
        }
    }
}

#[async_trait]
impl ConfigSource for StaticConfig {
    async fn config_for(&self, _: WorkspaceId, _: RoomId) -> Arc<SuggestionConfig> {
        match self.0.lock() {
            Ok(g) => g.clone(),
            Err(e) => e.into_inner().clone(),
        }
    }
    fn forget(&self, _: RoomId) {}
}
