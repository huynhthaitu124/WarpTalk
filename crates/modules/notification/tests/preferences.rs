#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{SystemClock, UserId};
use warptalk_notification::{MIGRATIONS, NotificationDeps, NotificationModule};
use warptalk_web::JwtKeys;

async fn setup() -> Option<(Router, JwtKeys)> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let module = NotificationModule::new(NotificationDeps {
        pool,
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
    });
    Some((module.router(), jwt))
}

async fn call(
    app: &Router,
    jwt: &JwtKeys,
    method: &str,
    user: Option<UserId>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri("/notifications/preferences");
    if let Some(u) = user {
        let t = jwt
            .issue_access(u, Uuid::now_v7(), false, Utc::now())
            .unwrap()
            .0;
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn requires_auth() {
    let Some((app, jwt)) = setup().await else {
        return;
    };
    let (st, _) = call(&app, &jwt, "GET", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = call(&app, &jwt, "PUT", None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_creates_default_row_once() {
    let Some((app, jwt)) = setup().await else {
        return;
    };
    let u = UserId::new();
    let (st, first) = call(&app, &jwt, "GET", Some(u), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(first["userId"], u.as_uuid().to_string());
    assert_eq!(first["notificationType"], "default");
    assert_eq!(first["emailEnabled"], true);
    assert_eq!(first["pushEnabled"], true);
    assert_eq!(first["inAppEnabled"], true);
    assert!(first["id"].is_string() && first["updatedAt"].is_string());
    let (_, second) = call(&app, &jwt, "GET", Some(u), None).await;
    assert_eq!(
        second["id"], first["id"],
        "GET must not create a second row"
    );
}

#[tokio::test]
async fn put_upserts_partially_and_isolates_users() {
    let Some((app, jwt)) = setup().await else {
        return;
    };
    let (a, b) = (UserId::new(), UserId::new());

    // PUT before any GET creates the row; omitted channels default on.
    let (st, v) = call(
        &app,
        &jwt,
        "PUT",
        Some(a),
        Some(json!({"emailEnabled": false})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["emailEnabled"], false);
    assert_eq!(v["pushEnabled"], true);
    let id = v["id"].clone();

    // Omitted fields keep their value.
    let (_, v) = call(
        &app,
        &jwt,
        "PUT",
        Some(a),
        Some(json!({"pushEnabled": false})),
    )
    .await;
    assert_eq!(v["id"], id);
    assert_eq!(v["emailEnabled"], false);
    assert_eq!(v["pushEnabled"], false);
    assert_eq!(v["inAppEnabled"], true);

    // Empty body changes nothing.
    let (st, v) = call(&app, &jwt, "PUT", Some(a), Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["emailEnabled"], false);
    assert_eq!(v["pushEnabled"], false);

    // Persisted for GET, and other users are unaffected.
    let (_, v) = call(&app, &jwt, "GET", Some(a), None).await;
    assert_eq!(v["emailEnabled"], false);
    let (_, other) = call(&app, &jwt, "GET", Some(b), None).await;
    assert_eq!(other["emailEnabled"], true);
    assert_ne!(other["id"], id);

    // Wrong types are rejected.
    let (st, _) = call(
        &app,
        &jwt,
        "PUT",
        Some(a),
        Some(json!({"emailEnabled": "no"})),
    )
    .await;
    assert!(st.is_client_error());
}
