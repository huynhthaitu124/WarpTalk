#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use warptalk_config::JwtConfig;
use warptalk_identity::VerificationMailer;
use warptalk_kernel::AppError;
use warptalk_web::JwtKeys;

#[derive(Default)]
struct CaptureMailer(Mutex<Vec<(String, String)>>);

#[async_trait::async_trait]
impl VerificationMailer for CaptureMailer {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push((email.to_owned(), token.to_owned()));
        Ok(())
    }
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn full_flow() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping e2e test");
        return;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(
        &pool,
        &[
            warptalk_db::OUTBOX_MIGRATIONS,
            warptalk_identity::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
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
    let mailer = Arc::new(CaptureMailer::default());
    let app =
        warptalk_api::build_router(pool, JwtKeys::from_config(&cfg), &cfg, &[], mailer.clone());

    let (s, _) = call(&app, "GET", "/health", None, None).await;
    assert_eq!(s, StatusCode::OK);

    let email = format!("e2e-{}@example.test", uuid::Uuid::new_v4().simple());
    let pw = "S3cret-pass!";
    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/auth/register",
        Some(json!({"email": email, "password": pw, "fullName": "E2E Owner"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let user_id = b["userId"].as_str().unwrap().to_owned();

    let token = mailer
        .0
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(e, _)| *e == email)
        .map(|(_, t)| t.clone())
        .unwrap();
    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");

    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": email, "password": pw})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let access = b["accessToken"].as_str().unwrap().to_owned();
    let refresh = b["refreshToken"].as_str().unwrap().to_owned();

    let ws_name = format!("E2E {}", uuid::Uuid::new_v4().simple());
    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": ws_name})),
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let ws_id = b["id"].as_str().unwrap().to_owned();

    let (s, b) = call(&app, "GET", "/api/v1/workspaces", None, Some(&access)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(
        b["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws_id.as_str())
    );

    let (s, b) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws_id}/members"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let owner = b["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["userId"] == user_id.as_str())
        .unwrap();
    assert_eq!(owner["email"], email.as_str());
    assert_eq!(owner["fullName"], "E2E Owner");

    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refreshToken": refresh})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let new_refresh = b["refreshToken"].as_str().unwrap().to_owned();

    let (s, b) = call(
        &app,
        "POST",
        "/api/v1/auth/logout",
        Some(json!({"refreshToken": new_refresh})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
}
