#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use warptalk_kernel::{RoomId, UserId, WorkspaceId};
use warptalk_settings::{
    MIGRATIONS, Scope, ScopeChain, SettingsStore, Validate, check_range, register_defaults,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Cfg {
    threshold: f32,
    label: String,
    nested: Nested,
    items: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Nested {
    a: u32,
    b: u32,
}

impl Default for Nested {
    fn default() -> Self {
        Self { a: 1, b: 2 }
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            label: "default".into(),
            nested: Nested::default(),
            items: vec![1, 2, 3],
        }
    }
}

impl Validate for Cfg {
    fn validate(&self) -> Result<(), String> {
        check_range("threshold", self.threshold, 0.0, 1.0)
    }
}

type AuditRow = (
    i64,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Option<Uuid>,
);

struct Env {
    pool: PgPool,
    redis_url: String,
}

async fn env() -> Option<Env> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("skipping: DATABASE_URL / REDIS_URL not set");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    Some(Env { pool, redis_url })
}

async fn store(env: &Env) -> SettingsStore {
    let conn = warptalk_bus::connect(&env.redis_url).await.unwrap();
    let s = SettingsStore::new(env.pool.clone(), Some(conn));
    register_defaults(&s);
    s
}

fn ns() -> String {
    format!("test_{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn precedence_merge_and_partial_override() {
    let Some(env) = env().await else { return };
    let s = store(&env).await;
    let ns = ns();
    s.register::<Cfg>(&ns);
    let (ws, room) = (WorkspaceId::new(), RoomId::new());
    let chain = ScopeChain::room(ws, room);

    // Nothing stored: pure defaults.
    assert_eq!(s.get::<Cfg>(&ns, chain).await.unwrap(), Cfg::default());

    s.put(
        Scope::Platform,
        &ns,
        json!({"label": "platform", "nested": {"a": 10}, "items": [9]}),
        None,
    )
    .await
    .unwrap();
    s.put(
        Scope::Workspace(ws),
        &ns,
        json!({"label": "ws", "nested": {"b": 20}}),
        None,
    )
    .await
    .unwrap();
    s.put(Scope::Room(room), &ns, json!({"threshold": 0.9}), None)
        .await
        .unwrap();

    let got: Cfg = s.get(&ns, chain).await.unwrap();
    assert_eq!(got.label, "ws"); // workspace beats platform
    assert_eq!(got.nested, Nested { a: 10, b: 20 }); // objects merge key-wise
    assert_eq!(got.items, vec![9]); // arrays replace
    assert!((got.threshold - 0.9).abs() < 1e-6); // room override

    // Partial override keeps defaults: other workspace sees platform + defaults only.
    let other: Cfg = s
        .get(&ns, ScopeChain::workspace(WorkspaceId::new()))
        .await
        .unwrap();
    assert_eq!(other.label, "platform");
    assert_eq!(other.nested, Nested { a: 10, b: 2 });
    assert!((other.threshold - 0.5).abs() < 1e-6);

    // Deleting the room override restores inheritance.
    assert!(s.delete(Scope::Room(room), &ns, None).await.unwrap());
    let got: Cfg = s.get(&ns, chain).await.unwrap();
    assert!((got.threshold - 0.5).abs() < 1e-6);
}

#[tokio::test]
async fn invalid_put_rejected() {
    let Some(env) = env().await else { return };
    let s = store(&env).await;
    let ns = ns();
    s.register::<Cfg>(&ns);

    let err = s
        .put(Scope::Platform, &ns, json!({"threshold": 7.0}), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SETTINGS_INVALID");
    let err = s
        .put(Scope::Platform, &ns, json!({"threshold": "high"}), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SETTINGS_INVALID");
    let err = s
        .put(Scope::Platform, &ns, json!([1]), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SETTINGS_INVALID");
    let err = s
        .put(Scope::Platform, "unregistered_ns", json!({}), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SETTINGS_UNKNOWN_NAMESPACE");

    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM public.settings_entries WHERE namespace = $1")
            .bind(&ns)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn invalid_stored_value_falls_back() {
    let Some(env) = env().await else { return };
    let s = SettingsStore::with_ttl(env.pool.clone(), None, Duration::from_millis(1));
    let ns = ns();
    s.register::<Cfg>(&ns);
    let ws = WorkspaceId::new();
    let chain = ScopeChain::workspace(ws);

    // No last-valid value: bad stored data yields defaults.
    sqlx::query(
        "INSERT INTO public.settings_entries (scope_kind, scope_id, namespace, value, version)
         VALUES ('workspace', $1, $2, $3, 1)",
    )
    .bind(ws.as_uuid())
    .bind(&ns)
    .bind(json!({"threshold": 42.0}))
    .execute(&env.pool)
    .await
    .unwrap();
    assert_eq!(s.get::<Cfg>(&ns, chain).await.unwrap(), Cfg::default());

    // Good value, then corrupted behind the store's back: last valid value wins.
    s.put(Scope::Workspace(ws), &ns, json!({"label": "good"}), None)
        .await
        .unwrap();
    assert_eq!(s.get::<Cfg>(&ns, chain).await.unwrap().label, "good");
    sqlx::query(
        "UPDATE public.settings_entries SET value = $1
         WHERE scope_kind = 'workspace' AND scope_id = $2 AND namespace = $3",
    )
    .bind(json!({"label": "bad", "threshold": -3.0}))
    .bind(ws.as_uuid())
    .bind(&ns)
    .execute(&env.pool)
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await; // let the 1 ms TTL lapse
    assert_eq!(s.get::<Cfg>(&ns, chain).await.unwrap().label, "good");
}

#[tokio::test]
async fn invalidation_across_two_instances() {
    let Some(env) = env().await else { return };
    let a = store(&env).await;
    let b = SettingsStore::new(env.pool.clone(), None); // long TTL: only Redis can refresh it
    let ns = ns();
    a.register::<Cfg>(&ns);
    b.register::<Cfg>(&ns);
    let _listener = b.spawn_invalidation_listener(&env.redis_url);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let chain = ScopeChain::workspace(WorkspaceId::new());
    assert_eq!(b.get::<Cfg>(&ns, chain).await.unwrap().label, "default"); // primes b's cache

    a.put(Scope::Platform, &ns, json!({"label": "changed"}), None)
        .await
        .unwrap();

    let start = Instant::now();
    loop {
        if b.get::<Cfg>(&ns, chain).await.unwrap().label == "changed" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "not propagated within 1s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn watch_receiver_gets_updates() {
    let Some(env) = env().await else { return };
    let s = store(&env).await;
    let ns = ns();
    s.register::<Cfg>(&ns);
    let (ws, room) = (WorkspaceId::new(), RoomId::new());

    let mut rx = s
        .watch::<Cfg>(&ns, ScopeChain::room(ws, room))
        .await
        .unwrap();
    assert_eq!(rx.borrow().label, "default");

    s.put(Scope::Room(room), &ns, json!({"label": "live"}), None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("no update within 1s")
        .unwrap();
    assert_eq!(rx.borrow().label, "live");

    // Unrelated room does not trigger a change.
    s.put(
        Scope::Room(RoomId::new()),
        &ns,
        json!({"label": "other"}),
        None,
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rx.changed())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn audit_row_written() {
    let Some(env) = env().await else { return };
    let s = store(&env).await;
    let ns = ns();
    s.register::<Cfg>(&ns);
    let actor = UserId::new();

    let v1 = s
        .put(Scope::Platform, &ns, json!({"label": "one"}), Some(actor))
        .await
        .unwrap();
    let v2 = s
        .put(Scope::Platform, &ns, json!({"label": "two"}), Some(actor))
        .await
        .unwrap();
    assert_eq!((v1.version, v2.version), (1, 2));

    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT version, old_value, new_value, actor FROM public.settings_audit
             WHERE namespace = $1 ORDER BY version",
    )
    .bind(&ns)
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, None);
    assert_eq!(rows[0].2, Some(json!({"label": "one"})));
    assert_eq!(rows[1].1, Some(json!({"label": "one"})));
    assert_eq!(rows[1].2, Some(json!({"label": "two"})));
    assert_eq!(rows[1].3, Some(actor.as_uuid()));

    // Append-only.
    let res = sqlx::query("DELETE FROM public.settings_audit WHERE namespace = $1")
        .bind(&ns)
        .execute(&env.pool)
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn builtin_namespaces_roundtrip() {
    let Some(env) = env().await else { return };
    let s = store(&env).await;
    // Platform rows are global; use a workspace scope to stay isolated.
    let ws = WorkspaceId::new();
    s.put(
        Scope::Workspace(ws),
        "suggestion",
        json!({"min_confidence": 0.8}),
        None,
    )
    .await
    .unwrap();
    let got: warptalk_settings::SuggestionSettings = s
        .get("suggestion", ScopeChain::workspace(ws))
        .await
        .unwrap();
    assert!((got.min_confidence - 0.8).abs() < 1e-6);
    assert_eq!(got.cooldown_seconds, 20);
    let err = s
        .put(
            Scope::Workspace(ws),
            "pipeline",
            json!({"chunk_duration_ms": 100}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SETTINGS_INVALID");
}
