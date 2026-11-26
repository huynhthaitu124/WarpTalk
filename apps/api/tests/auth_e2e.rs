//! Self-onboarding end to end through the real router: register -> verify -> login, forgot ->
//! reset (tokens read from the DEV_MAIL_CAPTURE keys), session revocation, Google not configured.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use redis::AsyncCommands;
use serde_json::{Value, json};
use tower::ServiceExt;
use warptalk_config::JwtConfig;
use warptalk_identity::DevMailCapture;
use warptalk_web::JwtKeys;

struct Env {
    app: Router,
    redis: redis::aio::ConnectionManager,
}

async fn env() -> Option<Env> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        eprintln!("DATABASE_URL / REDIS_URL not set; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 8).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_db::OUTBOX_MIGRATIONS,
            warptalk_settings::MIGRATIONS,
            warptalk_identity::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
            warptalk_room::MIGRATIONS,
            warptalk_meeting::MIGRATIONS,
            warptalk_transcript::MIGRATIONS,
            warptalk_knowledge::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
            warptalk_copilot::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let cfg = JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    };
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let mailer = DevMailCapture::from_flag(Some("1"), &redis_url)
        .unwrap()
        .unwrap();
    let app = warptalk_api::build_router(
        pool,
        JwtKeys::from_config(&cfg),
        &cfg,
        &[],
        Arc::new(mailer),
        Some(redis.clone()),
        warptalk_api::ApiOptions::default(),
    )
    .await;
    Some(Env { app, redis })
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/v1{uri}"));
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => {
            req = req.header("x-none", "1");
            req.body(Body::empty()).unwrap()
        }
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn captured(env: &Env, purpose: &str, email: &str) -> String {
    let mut conn = env.redis.clone();
    conn.get::<_, String>(DevMailCapture::key(purpose, email))
        .await
        .unwrap()
}

#[tokio::test]
async fn register_verify_forgot_reset_over_dev_capture() {
    let Some(env) = env().await else { return };
    let email = format!("onboard-{}@example.test", uuid::Uuid::new_v4().simple());
    let (pw, new_pw) = ("S3cret-pass!", "An0ther-pass!");

    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/register",
        Some(json!({"email": email, "password": pw, "fullName": "On Board"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let verify = captured(&env, "verify", &email).await;
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": verify})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (s, session) = call(
        &env.app,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": pw})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/forgot-password",
        Some(json!({"email": email})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let reset = captured(&env, "reset", &email).await;
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/reset-password",
        Some(json!({"token": reset, "newPassword": new_pw})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/reset-password",
        Some(json!({"token": reset, "newPassword": new_pw})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "reset token is single-use");

    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": session["refreshToken"]})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "BR-06: old sessions revoked");
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": pw})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": new_pw})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Redis-backed cooldown (BR-87): an immediate second request mails nothing new.
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/forgot-password",
        Some(json!({"email": email})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(captured(&env, "reset", &email).await, reset);
}

#[tokio::test]
async fn google_sign_in_is_503_without_client_ids() {
    if std::env::var("GOOGLE_CLIENT_IDS").is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    let Some(env) = env().await else { return };
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/google",
        Some(json!({"idToken": "x.y.z"})),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b["code"], "GOOGLE_NOT_CONFIGURED");
}
