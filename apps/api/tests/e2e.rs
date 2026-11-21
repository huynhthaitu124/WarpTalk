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
use warptalk_livekit::{LiveKitConfig, NoopRoomService};
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

/// Register, verify and log in a fresh user; returns `(user_id, access_token)`.
async fn new_user(app: &Router, mailer: &CaptureMailer, name: &str) -> (String, String) {
    let email = format!("e2e-{}@example.test", uuid::Uuid::new_v4().simple());
    let pw = "S3cret-pass!";
    let (s, b) = call(
        app,
        "POST",
        "/api/v1/auth/register",
        Some(json!({"email": email, "password": pw, "fullName": name})),
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
    let (s, _) = call(
        app,
        "POST",
        "/api/v1/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, b) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": email, "password": pw})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    (user_id, b["accessToken"].as_str().unwrap().to_owned())
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
            warptalk_settings::MIGRATIONS,
            warptalk_identity::MIGRATIONS,
            warptalk_workspace::MIGRATIONS,
            warptalk_room::MIGRATIONS,
            warptalk_meeting::MIGRATIONS,
            warptalk_transcript::MIGRATIONS,
            warptalk_knowledge::MIGRATIONS,
            warptalk_assistant::MIGRATIONS,
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
    let keys = JwtKeys::from_config(&cfg);
    let app = warptalk_api::build_router(
        pool.clone(),
        keys.clone(),
        &cfg,
        &[],
        mailer.clone(),
        None,
        warptalk_api::ApiOptions {
            livekit: Some(LiveKitConfig {
                url: "wss://lk.test".to_owned(),
                api_key: "APIkey".to_owned(),
                api_secret: "lk-secret-lk-secret-lk-secret-0001".to_owned(),
            }),
            room_service: Some(Arc::new(NoopRoomService)),
            redis_url: None,
            ai: None,
            vectors: None,
            vault: Some(Arc::new(
                warptalk_ai_plane::KeyVault::from_base64(
                    "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
                    1,
                )
                .unwrap(),
            )),
            key_validator: None,
        },
    )
    .await;
    let unconfigured = warptalk_api::build_router(
        pool.clone(),
        keys,
        &cfg,
        &[],
        mailer.clone(),
        None,
        warptalk_api::ApiOptions::default(),
    )
    .await;

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
        "/api/v1/translation-rooms",
        Some(json!({
            "workspaceId": ws_id,
            "title": "E2E Room",
            "sourceLanguage": "vi",
            "targetLanguages": ["en"]
        })),
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let room_id = b["id"].as_str().unwrap().to_owned();

    let (s, b) = call(
        &app,
        "POST",
        &format!("/api/v1/translation-rooms/{room_id}/start"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // meeting join: host gets a token with LiveKit configured, 503 without it
    let join = format!("/api/v1/meetings/{room_id}/join");
    let (s, b) = call(&app, "POST", &join, Some(json!({})), Some(&access)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(!b["token"].as_str().unwrap().is_empty());
    let (s, b) = call(&unconfigured, "POST", &join, Some(json!({})), Some(&access)).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b["code"], "LIVEKIT_NOT_CONFIGURED");

    // settings: owner PUT at workspace scope, effective GET merges; plain member is refused
    let put = format!("/api/v1/settings/suggestion?scope=workspace&id={ws_id}");
    let (s, b) = call(
        &app,
        "PUT",
        &put,
        Some(json!({"max_per_meeting": 7})),
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let eff = format!("/api/v1/settings/suggestion?effective=true&workspaceId={ws_id}");
    let (s, b) = call(&app, "GET", &eff, None, Some(&access)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["value"]["max_per_meeting"], 7);
    assert!(b["value"]["min_words"].is_number(), "defaults merged: {b}");
    let (member_id, member_access) = new_user(&app, &mailer, "E2E Member").await;
    sqlx::query(
        "INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) \
         VALUES ($1, $2, $3, '00000000-0000-7000-8000-000000000003', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(ws_id.parse::<uuid::Uuid>().unwrap())
    .bind(member_id.parse::<uuid::Uuid>().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let (s, _) = call(
        &app,
        "PUT",
        &put,
        Some(json!({"max_per_meeting": 1})),
        Some(&member_access),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, b) = call(&app, "GET", &eff, None, Some(&member_access)).await;
    assert_eq!(
        (s, &b["value"]["max_per_meeting"]),
        (StatusCode::OK, &json!(7))
    );
    // BYO AI config: OWNER/ADMIN only (workspace adapter), key never echoed.
    let ai_cfg = format!("/api/v1/ai/workspaces/{ws_id}/config");
    let (s, _) = call(&app, "GET", &ai_cfg, None, Some(&member_access)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/ai/workspaces/{ws_id}/credentials/openai"),
        Some(json!({"apiKey": "sk-e2e-member-0000"})),
        Some(&member_access),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, b) = call(&app, "GET", &ai_cfg, None, Some(&access)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["byoEnabled"], true);
    let (s, b) = call(
        &app,
        "PUT",
        &ai_cfg,
        Some(json!({"purposes": {"embedding": {"provider": "openai", "model": "text-embedding-3-small"}}})),
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["reindexRequired"], true);
    assert_eq!(b["purposes"]["embedding"]["status"], "invalid", "{b}");

    let (s, _) = call(
        &app,
        "GET",
        "/api/v1/settings/nope?scope=platform",
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Knowledge search: no index yet, so 200 with no chunks; non-members are refused.
    let (s, b) = call(
        &app,
        "GET",
        &format!("/api/v1/knowledge/search?workspaceId={ws_id}&q=budget"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["chunks"].as_array().map(Vec::len), Some(0), "{b}");
    let (s, _) = call(
        &app,
        "GET",
        &format!(
            "/api/v1/knowledge/search?workspaceId={}&q=budget",
            uuid::Uuid::new_v4()
        ),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, b) = call(
        &app,
        "GET",
        &format!("/api/v1/translation-rooms/{room_id}/audio-routes"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    let (s, b) = call(
        &app,
        "POST",
        &format!("/api/v1/translation-rooms/{room_id}/end"),
        None,
        Some(&access),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");

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
