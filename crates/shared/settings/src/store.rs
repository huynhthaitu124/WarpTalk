use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use futures::StreamExt;
use redis::aio::ConnectionManager;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;
use warptalk_db::map_sqlx_error;
use warptalk_kernel::{AppError, AppResult, UserId};

use crate::scope::{PLATFORM_SCOPE_ID, Scope, ScopeChain};
use crate::validate::{Validate, deep_merge};

/// Redis pub/sub channel carrying [`ChangeEvent`]s.
pub const CHANNEL: &str = "warptalk:settings:changed";

/// Default in-process cache TTL.
pub const DEFAULT_TTL: Duration = Duration::from_secs(30);

/// Published after every successful write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub scope: Scope,
    pub namespace: String,
    pub version: i64,
}

/// Outcome of [`SettingsStore::put`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutOutcome {
    pub version: i64,
}

type CacheKey = (String, ScopeChain);
type ValidatorFn = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

struct CacheEntry {
    value: Arc<Value>,
    fetched: Instant,
}

struct Inner {
    pool: PgPool,
    redis: Option<ConnectionManager>,
    ttl: Duration,
    cache: RwLock<HashMap<CacheKey, CacheEntry>>,
    last_valid: RwLock<HashMap<CacheKey, Value>>,
    validators: RwLock<HashMap<String, ValidatorFn>>,
    /// Bumped on every invalidation so an in-flight fetch cannot re-insert a stale value.
    generation: AtomicU64,
    events: broadcast::Sender<ChangeEvent>,
}

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// Layered runtime configuration store with a per-process cache.
/// Cheap to clone (shared state behind an `Arc`).
#[derive(Clone)]
pub struct SettingsStore {
    inner: Arc<Inner>,
}

impl SettingsStore {
    /// `redis` is used to publish invalidations; pass `None` to run single-process.
    pub fn new(pool: PgPool, redis: Option<ConnectionManager>) -> Self {
        Self::with_ttl(pool, redis, DEFAULT_TTL)
    }

    pub fn with_ttl(pool: PgPool, redis: Option<ConnectionManager>, ttl: Duration) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                pool,
                redis,
                ttl,
                cache: RwLock::new(HashMap::new()),
                last_valid: RwLock::new(HashMap::new()),
                validators: RwLock::new(HashMap::new()),
                generation: AtomicU64::new(0),
                events,
            }),
        }
    }

    /// Register the document type of a namespace; `put` rejects values that do not
    /// deserialize into `T` (with defaults filling gaps) or fail `T::validate`.
    pub fn register<T>(&self, ns: &str)
    where
        T: DeserializeOwned + Validate + 'static,
    {
        let f: ValidatorFn = Arc::new(|v: &Value| {
            let t: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            t.validate()
        });
        write(&self.inner.validators).insert(ns.to_string(), f);
    }

    /// Merged (platform, workspace, room) value of `ns`, deserialized into `T`.
    /// Invalid configuration never errors: falls back to the last valid value, then `T::default()`.
    /// Errors only when the database is unreachable and nothing is cached.
    pub async fn get<T>(&self, ns: &str, scope: ScopeChain) -> AppResult<T>
    where
        T: DeserializeOwned + Default + Validate,
    {
        let key: CacheKey = (ns.to_string(), scope);
        let merged = self.merged(&key).await?;
        match parse::<T>(&merged) {
            Ok(t) => {
                let unchanged = read(&self.inner.last_valid).get(&key) == Some(merged.as_ref());
                if !unchanged {
                    write(&self.inner.last_valid).insert(key, merged.as_ref().clone());
                }
                Ok(t)
            }
            Err(reason) => {
                tracing::warn!(namespace = ns, %reason, "invalid settings; using fallback");
                let previous = read(&self.inner.last_valid).get(&key).cloned();
                if let Some(prev) = previous
                    && let Ok(t) = parse::<T>(&prev)
                {
                    return Ok(t);
                }
                Ok(T::default())
            }
        }
    }

    /// Validate and store `value` (a partial document) at `scope`, bump the version,
    /// append an audit row and broadcast the change.
    pub async fn put(
        &self,
        scope: Scope,
        ns: &str,
        value: Value,
        actor: Option<UserId>,
    ) -> AppResult<PutOutcome> {
        if !value.is_object() {
            return Err(AppError::validation(
                "SETTINGS_INVALID",
                "settings value must be a JSON object",
            ));
        }
        let validator = read(&self.inner.validators)
            .get(ns)
            .cloned()
            .ok_or_else(|| {
                AppError::validation(
                    "SETTINGS_UNKNOWN_NAMESPACE",
                    format!("no settings registered for namespace `{ns}`"),
                )
            })?;

        let mut tx = self.inner.pool.begin().await.map_err(map_sqlx_error)?;
        lock_key(&mut tx, scope, ns).await?;

        // Validate the effective document: non-platform overrides sit on top of platform.
        let mut candidate = Value::Object(Default::default());
        if scope != Scope::Platform {
            let platform: Option<(Value,)> = sqlx::query_as(
                "SELECT value FROM public.settings_entries
                 WHERE scope_kind = 'platform' AND scope_id = $1 AND namespace = $2",
            )
            .bind(PLATFORM_SCOPE_ID)
            .bind(ns)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if let Some((p,)) = platform {
                candidate = p;
            }
        }
        deep_merge(&mut candidate, &value);
        validator(&candidate).map_err(|m| {
            AppError::validation("SETTINGS_INVALID", format!("invalid `{ns}` settings: {m}"))
        })?;

        let old: Option<(Value,)> = sqlx::query_as(
            "SELECT value FROM public.settings_entries
             WHERE scope_kind = $1 AND scope_id = $2 AND namespace = $3",
        )
        .bind(scope.kind())
        .bind(scope.id())
        .bind(ns)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;

        let actor_id = actor.map(|a| a.as_uuid());
        let (version,): (i64,) = sqlx::query_as(
            "INSERT INTO public.settings_entries
                 (scope_kind, scope_id, namespace, value, version, updated_by, updated_at)
             VALUES ($1, $2, $3, $4, 1, $5, now())
             ON CONFLICT (scope_kind, scope_id, namespace) DO UPDATE
                 SET value = EXCLUDED.value,
                     version = public.settings_entries.version + 1,
                     updated_by = EXCLUDED.updated_by,
                     updated_at = now()
             RETURNING version",
        )
        .bind(scope.kind())
        .bind(scope.id())
        .bind(ns)
        .bind(&value)
        .bind(actor_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;

        insert_audit(
            &mut tx,
            scope,
            ns,
            old.map(|o| o.0),
            Some(&value),
            version,
            actor_id,
        )
        .await?;
        tx.commit().await.map_err(map_sqlx_error)?;

        self.announce(ChangeEvent {
            scope,
            namespace: ns.to_string(),
            version,
        })
        .await;
        Ok(PutOutcome { version })
    }

    /// Remove the override at `scope` (the value inherits from lower-precedence scopes again).
    /// Returns whether a row existed.
    pub async fn delete(&self, scope: Scope, ns: &str, actor: Option<UserId>) -> AppResult<bool> {
        let mut tx = self.inner.pool.begin().await.map_err(map_sqlx_error)?;
        lock_key(&mut tx, scope, ns).await?;
        let old: Option<(Value, i64)> = sqlx::query_as(
            "DELETE FROM public.settings_entries
             WHERE scope_kind = $1 AND scope_id = $2 AND namespace = $3
             RETURNING value, version",
        )
        .bind(scope.kind())
        .bind(scope.id())
        .bind(ns)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx_error)?;
        let Some((old_value, old_version)) = old else {
            return Ok(false);
        };
        let version = old_version + 1;
        insert_audit(
            &mut tx,
            scope,
            ns,
            Some(old_value),
            None,
            version,
            actor.map(|a| a.as_uuid()),
        )
        .await?;
        tx.commit().await.map_err(map_sqlx_error)?;
        self.announce(ChangeEvent {
            scope,
            namespace: ns.to_string(),
            version,
        })
        .await;
        Ok(true)
    }

    /// Receiver holding the current value that is refreshed on every relevant change
    /// (and at least every TTL). The background task ends when all receivers are dropped.
    pub async fn watch<T>(&self, ns: &str, scope: ScopeChain) -> AppResult<watch::Receiver<T>>
    where
        T: DeserializeOwned + Default + Validate + PartialEq + Send + Sync + 'static,
    {
        let initial: T = self.get(ns, scope).await?;
        let (tx, rx) = watch::channel(initial);
        let mut events = self.inner.events.subscribe();
        let store = self.clone();
        let ns = ns.to_string();
        let ttl = self.inner.ttl;
        tokio::spawn(async move {
            loop {
                let refresh = tokio::select! {
                    _ = tx.closed() => break,
                    ev = events.recv() => match ev {
                        Ok(ev) => ev.namespace == ns && scope.is_affected_by(&ev.scope),
                        Err(broadcast::error::RecvError::Lagged(_)) => true,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    _ = tokio::time::sleep(ttl) => true,
                };
                if !refresh {
                    continue;
                }
                match store.get::<T>(&ns, scope).await {
                    Ok(next) => {
                        tx.send_if_modified(|cur| {
                            if *cur == next {
                                false
                            } else {
                                *cur = next;
                                true
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(namespace = %ns, error = %e, "settings refresh failed")
                    }
                }
            }
        });
        Ok(rx)
    }

    /// Subscribe to [`CHANNEL`] and invalidate the local cache on every message, so changes
    /// made by other replicas apply immediately. Reconnects on failure (clearing the whole
    /// cache, since messages may have been missed).
    pub fn spawn_invalidation_listener(&self, redis_url: &str) -> JoinHandle<()> {
        let store = self.clone();
        let url = redis_url.to_string();
        tokio::spawn(async move {
            loop {
                match warptalk_bus::subscribe(&url, &[CHANNEL.to_string()]).await {
                    Ok(stream) => {
                        // Anything that changed while disconnected is unknown.
                        store.invalidate_all();
                        let mut stream = Box::pin(stream);
                        while let Some((_, payload)) = stream.next().await {
                            match serde_json::from_str::<ChangeEvent>(&payload) {
                                Ok(ev) => store.apply_event(ev),
                                Err(e) => {
                                    tracing::warn!(error = %e, "bad settings change event");
                                    store.invalidate_all();
                                }
                            }
                        }
                        tracing::warn!("settings invalidation stream ended; reconnecting");
                    }
                    Err(e) => tracing::warn!(error = %e, "settings invalidation subscribe failed"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }

    async fn announce(&self, ev: ChangeEvent) {
        self.apply_event(ev.clone());
        if let Some(conn) = &self.inner.redis {
            let mut conn = conn.clone();
            if let Err(e) = warptalk_bus::publish_json(&mut conn, CHANNEL, &ev).await {
                tracing::warn!(error = %e, "failed to publish settings change; replicas rely on TTL");
            }
        }
    }

    /// Drop affected cache entries and wake local watchers.
    fn apply_event(&self, ev: ChangeEvent) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        write(&self.inner.cache)
            .retain(|(ns, chain), _| !(*ns == ev.namespace && chain.is_affected_by(&ev.scope)));
        let _ = self.inner.events.send(ev);
    }

    fn invalidate_all(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        write(&self.inner.cache).clear();
    }

    async fn merged(&self, key: &CacheKey) -> AppResult<Arc<Value>> {
        let fresh = |e: &CacheEntry| e.fetched.elapsed() < self.inner.ttl;
        let stale = {
            let cache = read(&self.inner.cache);
            match cache.get(key) {
                Some(e) if fresh(e) => return Ok(e.value.clone()),
                Some(e) => Some(e.value.clone()),
                None => None,
            }
        };
        let generation = self.inner.generation.load(Ordering::SeqCst);
        match self.load(&key.0, key.1).await {
            Ok(v) => {
                let v = Arc::new(v);
                if self.inner.generation.load(Ordering::SeqCst) == generation {
                    write(&self.inner.cache).insert(
                        key.clone(),
                        CacheEntry {
                            value: v.clone(),
                            fetched: Instant::now(),
                        },
                    );
                }
                Ok(v)
            }
            Err(e) => match stale {
                Some(v) => {
                    tracing::warn!(namespace = %key.0, error = %e, "settings load failed; serving stale");
                    Ok(v)
                }
                None => Err(e),
            },
        }
    }

    async fn load(&self, ns: &str, chain: ScopeChain) -> AppResult<Value> {
        let rows: Vec<(String, Value)> = sqlx::query_as(
            "SELECT scope_kind, value FROM public.settings_entries
             WHERE namespace = $1
               AND ((scope_kind = 'platform' AND scope_id = $2)
                 OR (scope_kind = 'workspace' AND scope_id = $3)
                 OR (scope_kind = 'room' AND scope_id = $4))",
        )
        .bind(ns)
        .bind(PLATFORM_SCOPE_ID)
        .bind(chain.workspace.map_or(Uuid::nil(), |w| w.as_uuid()))
        .bind(chain.room.map_or(Uuid::nil(), |r| r.as_uuid()))
        .fetch_all(&self.inner.pool)
        .await
        .map_err(map_sqlx_error)?;

        let mut merged = Value::Object(Default::default());
        for kind in ["platform", "workspace", "room"] {
            for (_, v) in rows.iter().filter(|(k, _)| k == kind) {
                deep_merge(&mut merged, v);
            }
        }
        Ok(merged)
    }
}

fn parse<T: DeserializeOwned + Validate>(v: &Value) -> Result<T, String> {
    let t: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    t.validate()?;
    Ok(t)
}

/// Serialise concurrent writers of the same (scope, namespace) for the transaction's life.
async fn lock_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: Scope,
    ns: &str,
) -> AppResult<()> {
    let key = format!("settings:{}:{}:{}", scope.kind(), scope.id(), ns);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(&mut **tx)
        .await
        .map_err(map_sqlx_error)?;
    Ok(())
}

async fn insert_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: Scope,
    ns: &str,
    old: Option<Value>,
    new: Option<&Value>,
    version: i64,
    actor: Option<Uuid>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO public.settings_audit
             (id, scope_kind, scope_id, namespace, old_value, new_value, version, actor)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(scope.kind())
    .bind(scope.id())
    .bind(ns)
    .bind(old)
    .bind(new)
    .bind(version)
    .bind(actor)
    .execute(&mut **tx)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}
