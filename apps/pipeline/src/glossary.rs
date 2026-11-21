//! Glossary feed for MT: [`GlossaryProvider`] backed by the Redis snapshots the transcript
//! module writes (the pipeline has no database).
//!
//! Keys (written by `warptalk-transcript` on every glossary change):
//! - `glossary:workspace:{workspaceId}` and `glossary:global`:
//!   `{"version": n, "terms": [{"source","target","sourceLang","targetLang","caseSensitive"}]}`
//! - channel `warptalk:glossary:changed`: `{"scope":"workspace"|"global","id":<ws id|null>,"version":n}`
//! - `translationRoom:{roomId}:workspace`: workspace id of a room. Neither the audio-routes
//!   snapshot nor the room events carry it, so the api writes this key (a decorator over the
//!   room module's event publisher, see `apps/api/src/wiring.rs`) whenever it publishes a
//!   room's routes.
//!
//! Terms are cached in process per workspace and for the global list; the merged list per room
//! (workspace terms win over global, `warptalk_mt::merge_glossaries`) is cached too. A change
//! message drops the affected entries, the next call reloads them lazily. Entries also expire
//! after `ttl` so a missed message (subscriber reconnecting) heals on its own.
//!
//! Limits of `GlossaryProvider`: it is asked per room, not per language pair, so the snapshot
//! languages and `caseSensitive` are not used here; every term of the room's glossaries is
//! offered and the MT stage selects the ones that occur in the text.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use tracing::{debug, warn};
use warptalk_mt::{GlossaryProvider, GlossaryTerm, merge_glossaries};

pub const GLOSSARY_CHANGED_CHANNEL: &str = "warptalk:glossary:changed";
pub const GLOSSARY_GLOBAL_KEY: &str = "glossary:global";

pub fn workspace_key(ws: &str) -> String {
    format!("glossary:workspace:{ws}")
}

pub fn room_workspace_key(room: &str) -> String {
    format!("translationRoom:{room}:workspace")
}

/// Read-only key/value access (Redis in production, a map in tests).
#[async_trait]
pub trait KvSource: Send + Sync {
    /// `Ok(None)` when the key does not exist; `Err` on transport failure.
    async fn get(&self, key: &str) -> Result<Option<String>, String>;
}

pub struct RedisKv(pub ConnectionManager);

#[async_trait]
impl KvSource for RedisKv {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let mut conn = self.0.clone();
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotJson {
    #[serde(default)]
    terms: Vec<SnapshotTermJson>,
}

#[derive(Debug, Deserialize)]
struct SnapshotTermJson {
    source: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct ChangeJson {
    scope: String,
    id: Option<String>,
}

fn parse_snapshot(raw: &str) -> Vec<GlossaryTerm> {
    match serde_json::from_str::<SnapshotJson>(raw) {
        Ok(s) => s
            .terms
            .into_iter()
            .filter(|t| !t.source.trim().is_empty())
            .map(|t| GlossaryTerm::new(t.source, t.target))
            .collect(),
        Err(e) => {
            warn!(error = %e, "ignoring malformed glossary snapshot");
            Vec::new()
        }
    }
}

struct Entry<T> {
    value: T,
    at: Instant,
}

#[derive(Default)]
struct State {
    global: Option<Entry<Arc<Vec<GlossaryTerm>>>>,
    workspaces: HashMap<String, Entry<Arc<Vec<GlossaryTerm>>>>,
    /// room -> workspace id (`None`: not known yet; retried after `ttl_unknown`).
    room_ws: HashMap<String, Entry<Option<String>>>,
    merged: HashMap<String, Entry<Arc<Vec<GlossaryTerm>>>>,
}

pub struct RedisGlossaryProvider {
    kv: Arc<dyn KvSource>,
    ttl: Duration,
    /// How long "room has no known workspace" is remembered before asking again.
    ttl_unknown: Duration,
    state: RwLock<State>,
}

const MAX_ROOMS: usize = 2_048;

impl RedisGlossaryProvider {
    pub fn new(kv: Arc<dyn KvSource>) -> Self {
        Self::with_ttls(kv, Duration::from_secs(600), Duration::from_secs(3))
    }

    pub fn with_ttls(kv: Arc<dyn KvSource>, ttl: Duration, ttl_unknown: Duration) -> Self {
        Self {
            kv,
            ttl,
            ttl_unknown,
            state: RwLock::new(State::default()),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drops what a change message invalidates. Unknown scopes drop everything.
    pub fn on_change(&self, payload: &str) {
        let change: Option<ChangeJson> = serde_json::from_str(payload).ok();
        let mut s = self.write();
        match change {
            Some(ChangeJson {
                scope,
                id: Some(id),
            }) if scope == "workspace" => {
                s.workspaces.remove(&id);
            }
            Some(ChangeJson { scope, .. }) if scope == "global" => s.global = None,
            _ => {
                s.global = None;
                s.workspaces.clear();
            }
        }
        s.merged.clear();
    }

    /// Drops every cached value (after a pub/sub reconnect: messages may have been missed).
    pub fn invalidate_all(&self) {
        let mut s = self.write();
        s.global = None;
        s.workspaces.clear();
        s.merged.clear();
    }

    fn fresh<T>(&self, e: &Entry<T>) -> bool {
        e.at.elapsed() < self.ttl
    }

    async fn load_list(&self, key: &str) -> Option<Arc<Vec<GlossaryTerm>>> {
        match self.kv.get(key).await {
            Ok(raw) => Some(Arc::new(
                raw.as_deref().map(parse_snapshot).unwrap_or_default(),
            )),
            Err(e) => {
                warn!(key, error = %e, "glossary snapshot unavailable");
                None
            }
        }
    }

    async fn global(&self) -> Option<Arc<Vec<GlossaryTerm>>> {
        if let Some(e) = self.read().global.as_ref().filter(|e| self.fresh(e)) {
            return Some(e.value.clone());
        }
        let v = self.load_list(GLOSSARY_GLOBAL_KEY).await?;
        self.write().global = Some(Entry {
            value: v.clone(),
            at: Instant::now(),
        });
        Some(v)
    }

    async fn workspace(&self, ws: &str) -> Option<Arc<Vec<GlossaryTerm>>> {
        if let Some(e) = self.read().workspaces.get(ws).filter(|e| self.fresh(e)) {
            return Some(e.value.clone());
        }
        let v = self.load_list(&workspace_key(ws)).await?;
        self.write().workspaces.insert(
            ws.to_owned(),
            Entry {
                value: v.clone(),
                at: Instant::now(),
            },
        );
        Some(v)
    }

    async fn room_workspace(&self, room: &str) -> Option<String> {
        if let Some(e) = self.read().room_ws.get(room) {
            let limit = if e.value.is_some() {
                self.ttl
            } else {
                self.ttl_unknown
            };
            if e.at.elapsed() < limit {
                return e.value.clone();
            }
        }
        let ws = match self.kv.get(&room_workspace_key(room)).await {
            Ok(v) => v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            Err(e) => {
                warn!(room, error = %e, "room workspace lookup failed");
                return None;
            }
        };
        let mut s = self.write();
        if s.room_ws.len() >= MAX_ROOMS {
            let ttl = self.ttl;
            s.room_ws.retain(|_, e| e.at.elapsed() < ttl);
            s.merged.retain(|_, e| e.at.elapsed() < ttl);
        }
        s.room_ws.insert(
            room.to_owned(),
            Entry {
                value: ws.clone(),
                at: Instant::now(),
            },
        );
        ws
    }
}

#[async_trait]
impl GlossaryProvider for RedisGlossaryProvider {
    async fn terms(&self, room: &str) -> Arc<Vec<GlossaryTerm>> {
        if let Some(e) = self.read().merged.get(room).filter(|e| self.fresh(e)) {
            return e.value.clone();
        }
        let ws = self.room_workspace(room).await;
        let global = self.global().await;
        let workspace = match &ws {
            Some(ws) => self.workspace(ws).await,
            None => Some(Arc::new(Vec::new())),
        };
        let (Some(global), Some(workspace)) = (global, workspace) else {
            // Redis hiccup: translate without glossary rather than stall; not cached.
            return Arc::new(Vec::new());
        };
        let merged = Arc::new(merge_glossaries(&global, &workspace));
        // A room whose workspace is not known yet gets global terms only, and must not keep
        // them past the unknown-room window once the workspace shows up.
        if ws.is_some() {
            self.write().merged.insert(
                room.to_owned(),
                Entry {
                    value: merged.clone(),
                    at: Instant::now(),
                },
            );
        }
        merged
    }
}

/// Subscribes to [`GLOSSARY_CHANGED_CHANNEL`] and invalidates `provider` on every message.
/// Reconnects with backoff; every (re)connect drops the cache since messages may have been lost.
pub fn spawn_invalidation_listener(
    provider: Arc<RedisGlossaryProvider>,
    redis_url: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let channels = [GLOSSARY_CHANGED_CHANNEL.to_owned()];
        let mut backoff = Duration::from_millis(500);
        loop {
            match warptalk_bus::subscribe(&redis_url, &channels).await {
                Ok(stream) => {
                    backoff = Duration::from_millis(500);
                    provider.invalidate_all();
                    let mut stream = std::pin::pin!(stream);
                    while let Some((_, payload)) = stream.next().await {
                        debug!(%payload, "glossary changed");
                        provider.on_change(&payload);
                    }
                    warn!("glossary subscription ended; reconnecting");
                }
                Err(e) => warn!(error = %e, "glossary subscription failed"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(15));
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeKv {
        data: Mutex<HashMap<String, String>>,
        gets: AtomicUsize,
    }

    impl FakeKv {
        fn set(&self, k: &str, v: &str) {
            self.data.lock().unwrap().insert(k.into(), v.into());
        }
    }

    #[async_trait]
    impl KvSource for FakeKv {
        async fn get(&self, key: &str) -> Result<Option<String>, String> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Ok(self.data.lock().unwrap().get(key).cloned())
        }
    }

    fn snap(version: i64, terms: &[(&str, &str)]) -> String {
        let t: Vec<_> = terms
            .iter()
            .map(|(s, t)| {
                serde_json::json!({"source": s, "target": t, "sourceLang": "en",
                    "targetLang": "vi", "caseSensitive": false})
            })
            .collect();
        serde_json::json!({"version": version, "terms": t}).to_string()
    }

    fn pairs(v: &[GlossaryTerm]) -> Vec<(&str, &str)> {
        v.iter()
            .map(|t| (t.source.as_str(), t.target.as_str()))
            .collect()
    }

    fn setup() -> (Arc<FakeKv>, RedisGlossaryProvider) {
        let kv = Arc::new(FakeKv::default());
        (kv.clone(), RedisGlossaryProvider::new(kv))
    }

    #[tokio::test]
    async fn loads_snapshots_and_workspace_wins_over_global() {
        let (kv, p) = setup();
        kv.set("translationRoom:r1:workspace", "ws1");
        kv.set(
            GLOSSARY_GLOBAL_KEY,
            &snap(1, &[("Kubernetes", "K8s"), ("API", "giao dien")]),
        );
        kv.set(
            &workspace_key("ws1"),
            &snap(1, &[("api", "API"), ("Acme", "Acme")]),
        );
        let terms = p.terms("r1").await;
        assert_eq!(
            pairs(&terms),
            vec![("api", "API"), ("Acme", "Acme"), ("Kubernetes", "K8s")]
        );
        // Served from cache the second time.
        let gets = kv.gets.load(Ordering::SeqCst);
        assert_eq!(p.terms("r1").await.len(), 3);
        assert_eq!(kv.gets.load(Ordering::SeqCst), gets);
    }

    #[tokio::test]
    async fn change_message_invalidates_only_what_changed() {
        let (kv, p) = setup();
        kv.set("translationRoom:r1:workspace", "ws1");
        kv.set("translationRoom:r2:workspace", "ws2");
        kv.set(&workspace_key("ws1"), &snap(1, &[("a", "1")]));
        kv.set(&workspace_key("ws2"), &snap(1, &[("b", "2")]));
        assert_eq!(pairs(&p.terms("r1").await), vec![("a", "1")]);
        assert_eq!(pairs(&p.terms("r2").await), vec![("b", "2")]);

        kv.set(&workspace_key("ws1"), &snap(2, &[("a", "1"), ("c", "3")]));
        kv.set(&workspace_key("ws2"), &snap(2, &[("b", "changed")]));
        // Stale until the message arrives.
        assert_eq!(p.terms("r1").await.len(), 1);
        p.on_change(r#"{"scope":"workspace","id":"ws1","version":2}"#);
        assert_eq!(pairs(&p.terms("r1").await), vec![("a", "1"), ("c", "3")]);
        // ws2's own snapshot is still cached (merged cache is rebuilt from per-workspace cache).
        assert_eq!(pairs(&p.terms("r2").await), vec![("b", "2")]);

        kv.set(GLOSSARY_GLOBAL_KEY, &snap(3, &[("g", "G")]));
        p.on_change(r#"{"scope":"global","id":null,"version":3}"#);
        assert_eq!(
            pairs(&p.terms("r1").await),
            vec![("a", "1"), ("c", "3"), ("g", "G")]
        );
        // Garbage drops everything.
        p.on_change("not json");
        assert_eq!(
            pairs(&p.terms("r2").await),
            vec![("b", "changed"), ("g", "G")]
        );
    }

    #[tokio::test]
    async fn room_without_workspace_gets_global_and_picks_workspace_up_later() {
        let kv = Arc::new(FakeKv::default());
        let p = RedisGlossaryProvider::with_ttls(
            kv.clone(),
            Duration::from_secs(600),
            Duration::from_millis(0),
        );
        kv.set(GLOSSARY_GLOBAL_KEY, &snap(1, &[("g", "G")]));
        assert_eq!(pairs(&p.terms("r1").await), vec![("g", "G")]);
        kv.set("translationRoom:r1:workspace", "ws1");
        kv.set(&workspace_key("ws1"), &snap(1, &[("w", "W")]));
        assert_eq!(pairs(&p.terms("r1").await), vec![("w", "W"), ("g", "G")]);
    }

    #[tokio::test]
    async fn missing_or_malformed_snapshots_yield_no_terms() {
        let (kv, p) = setup();
        assert!(p.terms("nobody").await.is_empty());
        kv.set("translationRoom:r1:workspace", "ws1");
        kv.set(&workspace_key("ws1"), "{broken");
        assert!(p.terms("r1").await.is_empty());
    }
}
