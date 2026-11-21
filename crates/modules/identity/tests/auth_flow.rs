#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tower::ServiceExt;
use warptalk_config::JwtConfig;
use warptalk_identity::{
    DevMailCapture, IdentityDeps, IdentityModule, MIGRATIONS, Policy, VerificationMailer,
};
use warptalk_kernel::{AppError, FixedClock};
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

impl CaptureMailer {
    fn token_for(&self, email: &str) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(e, _)| e == email)
            .map(|(_, t)| t.clone())
            .unwrap()
    }
}

struct Env {
    app: Router,
    clock: Arc<FixedClock>,
    mailer: Arc<CaptureMailer>,
    jwt: JwtKeys,
    pool: sqlx::PgPool,
    module: IdentityModule,
}

async fn setup() -> Option<Env> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_db::OUTBOX_MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let mailer = Arc::new(CaptureMailer::default());
    let module = IdentityModule::new(IdentityDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: clock.clone(),
        mailer: mailer.clone(),
        policy: Policy::default(),
    });
    Some(Env {
        app: module.router(),
        clock,
        mailer,
        jwt,
        pool,
        module,
    })
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
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

fn fresh_email() -> String {
    format!("user-{}@example.test", uuid::Uuid::new_v4().simple())
}

const PW: &str = "S3cret-pass!";

async fn register(env: &Env, email: &str) -> String {
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/register",
        Some(json!({"email": email, "password": PW, "fullName": "Test User"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    b["userId"].as_str().unwrap().to_owned()
}

async fn register_verified(env: &Env) -> String {
    let email = fresh_email();
    register(env, &email).await;
    let token = env.mailer.token_for(&email);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    email
}

async fn login(env: &Env, email: &str, pw: &str) -> (StatusCode, Value) {
    call(
        &env.app,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": pw})),
        None,
    )
    .await
}

#[tokio::test]
async fn register_verify_login() {
    let Some(env) = setup().await else { return };
    let email = fresh_email();
    let upper = email.to_uppercase();
    let user_id = register(&env, &upper).await;

    // Duplicate (case-insensitive) -> 409.
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/register",
        Some(json!({"email": email, "password": PW, "fullName": "X"})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::CONFLICT, Some("EMAIL_TAKEN"))
    );

    // Short password -> 400.
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/register",
        Some(json!({"email": fresh_email(), "password": "short", "fullName": "X"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Refused before verification (BR-02).
    let (s, b) = login(&env, &email, PW).await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("EMAIL_NOT_VERIFIED"))
    );

    // Same 401 for wrong password and unknown email.
    let (s1, b1) = login(&env, &email, "wrong-password").await;
    let (s2, b2) = login(&env, &fresh_email(), PW).await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(b1, b2);
    assert_eq!(b1["code"], "INVALID_CREDENTIALS");

    // Verify is single-use (BR-90).
    let token = env.mailer.token_for(&email);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("INVALID_TOKEN"))
    );

    let (s, b) = login(&env, &email, PW).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["userId"], user_id);
    assert_eq!(b["email"], email);
    assert!(b["accessTokenExpiresAt"].is_string());
    let claims = env.jwt.verify(b["accessToken"].as_str().unwrap()).unwrap();
    assert!(!claims.sa);
    assert_eq!(claims.sub.to_string(), user_id);
}

#[tokio::test]
async fn verification_token_expires() {
    let Some(env) = setup().await else { return };
    let email = fresh_email();
    register(&env, &email).await;
    let token = env.mailer.token_for(&email);
    env.clock.set(Utc::now() + Duration::minutes(61));
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("INVALID_TOKEN"))
    );
}

#[tokio::test]
async fn lockout_and_unlock() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    for _ in 0..5 {
        let (s, b) = login(&env, &email, "bad-password").await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::UNAUTHORIZED, Some("INVALID_CREDENTIALS"))
        );
    }
    // Locked even with the right password.
    let (s, b) = login(&env, &email, PW).await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("ACCOUNT_LOCKED"))
    );

    env.clock.set(Utc::now() + Duration::minutes(16));
    let (s, b) = login(&env, &email, PW).await;
    assert_eq!(s, StatusCode::OK, "{b}");
}

#[tokio::test]
async fn failure_counter_resets_on_success() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    for _ in 0..4 {
        login(&env, &email, "bad-password").await;
    }
    assert_eq!(login(&env, &email, PW).await.0, StatusCode::OK);
    for _ in 0..4 {
        login(&env, &email, "bad-password").await;
    }
    assert_eq!(login(&env, &email, PW).await.0, StatusCode::OK);
}

#[tokio::test]
async fn refresh_rotation_and_reuse_detection() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    let (_, l) = login(&env, &email, PW).await;
    let r0 = l["refreshToken"].as_str().unwrap().to_owned();

    let (s, r1b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r0})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r1b}");
    let r1 = r1b["refreshToken"].as_str().unwrap().to_owned();
    assert_ne!(r0, r1);
    let c0 = env.jwt.verify(l["accessToken"].as_str().unwrap()).unwrap();
    let c1 = env
        .jwt
        .verify(r1b["accessToken"].as_str().unwrap())
        .unwrap();
    assert_eq!(c0.sid, c1.sid, "same family");

    let (s, r2b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r1})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let r2 = r2b["refreshToken"].as_str().unwrap().to_owned();

    // Replaying an already rotated token revokes the whole family.
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r0})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("REFRESH_TOKEN_REUSED"))
    );
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r2})),
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "newest token must stop working too"
    );

    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": "nonsense"})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("INVALID_REFRESH_TOKEN"))
    );
}

#[tokio::test]
async fn refresh_token_expires() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    let (_, l) = login(&env, &email, PW).await;
    env.clock.set(Utc::now() + Duration::days(31));
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": l["refreshToken"]})),
        None,
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("REFRESH_TOKEN_EXPIRED"))
    );
}

#[tokio::test]
async fn logout_revokes_family() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    let (_, l) = login(&env, &email, PW).await;
    let r0 = l["refreshToken"].as_str().unwrap().to_owned();
    let (_, r1b) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r0})),
        None,
    )
    .await;
    let r1 = r1b["refreshToken"].as_str().unwrap().to_owned();

    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/logout",
        Some(json!({"refreshToken": r1})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": r1})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Unknown token logout is still 204 (idempotent).
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/logout",
        Some(json!({"refreshToken": "nope"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn profile_and_settings() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;

    for (m, u) in [("GET", "/me/profile"), ("GET", "/me/settings")] {
        let (s, _) = call(&env.app, m, u, None, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{u}");
    }
    let (s, _) = call(
        &env.app,
        "PUT",
        "/me/profile",
        Some(json!({"fullName":"x","preferredLanguage":"en"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (_, l) = login(&env, &email, PW).await;
    let t = l["accessToken"].as_str().unwrap();

    let (s, p) = call(&env.app, "GET", "/me/profile", None, Some(t)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["fullName"], "Test User");
    assert_eq!(p["preferredLanguage"], "vi-VN");

    let (s, p) = call(
        &env.app,
        "PUT",
        "/me/profile",
        Some(json!({"fullName":"New Name","avatarUrl":"https://img.example/a.png","preferredLanguage":"en-US"})),
        Some(t),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let (_, p) = call(&env.app, "GET", "/me/profile", None, Some(t)).await;
    assert_eq!(p["fullName"], "New Name");
    assert_eq!(p["avatarUrl"], "https://img.example/a.png");
    assert_eq!(p["preferredLanguage"], "en-US");
    assert_eq!(p["timezone"], "Asia/Ho_Chi_Minh");

    let (s, b) = call(
        &env.app,
        "PUT",
        "/me/profile",
        Some(json!({"fullName":"N","preferredLanguage":"not a tag"})),
        Some(t),
    )
    .await;
    assert_eq!(
        (s, b["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("VALIDATION_ERROR"))
    );
    let (s, _) = call(
        &env.app,
        "PUT",
        "/me/profile",
        Some(json!({"fullName":"N","preferredLanguage":"en","timezone":"Mars"})),
        Some(t),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, st) = call(&env.app, "GET", "/me/settings", None, Some(t)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["autoTranslateEnabled"], true);
    assert_eq!(st["defaultSpeakLanguage"], "vi-VN");
    assert_eq!(st["defaultAudioInput"], Value::Null);

    let (s, b) = call(
        &env.app,
        "PUT",
        "/me/settings",
        Some(json!({"defaultAudioInput":"mic-1","defaultAudioOutput":"spk-1","autoTranslateEnabled":false,"defaultListenLanguage":"ja-JP"})),
        Some(t),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (_, st) = call(&env.app, "GET", "/me/settings", None, Some(t)).await;
    assert_eq!(st["defaultAudioInput"], "mic-1");
    assert_eq!(st["defaultAudioOutput"], "spk-1");
    assert_eq!(st["autoTranslateEnabled"], false);
    assert_eq!(st["defaultSpeakLanguage"], "vi-VN");
    assert_eq!(st["defaultListenLanguage"], "ja-JP");

    let (s, _) = call(
        &env.app,
        "PUT",
        "/me/settings",
        Some(json!({"autoTranslateEnabled":true,"defaultSpeakLanguage":"xx_YY"})),
        Some(t),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn queries_facade_and_system_admin_claim() {
    let Some(env) = setup().await else { return };
    let email = register_verified(&env).await;
    let q = env.module.queries();
    let found = q
        .find_by_email(&email.to_uppercase())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.email, email);
    assert_eq!(found.full_name, "Test User");
    let by_id = q.user_summary(found.id).await.unwrap().unwrap();
    assert_eq!(by_id, found);
    assert!(q.find_by_email(&fresh_email()).await.unwrap().is_none());
    assert!(
        q.user_summary(warptalk_kernel::UserId::new())
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query(
        "INSERT INTO auth.user_roles (id, user_id, role_id, assigned_at)
         SELECT gen_random_uuid(), $1, id, now() FROM auth.roles WHERE name = 'SYSTEM_ADMIN'",
    )
    .bind(found.id.as_uuid())
    .execute(&env.pool)
    .await
    .unwrap();
    let (_, l) = login(&env, &email, PW).await;
    assert!(
        env.jwt
            .verify(l["accessToken"].as_str().unwrap())
            .unwrap()
            .sa
    );
}

#[tokio::test]
async fn me_returns_account_overview() {
    let Some(env) = setup().await else { return };
    let email = fresh_email();
    let user_id = register(&env, &email).await;
    let token = env.mailer.token_for(&email);
    let (s, _) = call(
        &env.app,
        "POST",
        "/auth/verify-email",
        Some(json!({ "token": token })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, l) = login(&env, &email, PW).await;
    let at = l["accessToken"].as_str().unwrap().to_owned();

    let (s, me) = call(&env.app, "GET", "/me", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(me["email"].is_null());

    let (s, me) = call(&env.app, "GET", "/me", None, Some(&at)).await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["userId"], user_id);
    assert_eq!(me["email"], email);
    assert_eq!(me["fullName"], "Test User");
    assert!(me["avatarUrl"].is_null());
    assert!(me["preferredLanguage"].is_string());
    assert!(me["timezone"].is_string());
    assert_eq!(me["emailVerified"], true);
    assert_eq!(me["systemAdmin"], false);
}

#[tokio::test]
async fn dev_mail_capture_is_opt_in_and_stores_latest_raw_token() {
    let Ok(url) = std::env::var("REDIS_URL") else {
        eprintln!("REDIS_URL not set; skipping");
        return;
    };
    assert!(DevMailCapture::from_flag(None, &url).unwrap().is_none());
    assert!(
        DevMailCapture::from_flag(Some("true"), &url)
            .unwrap()
            .is_none()
    );
    let m = DevMailCapture::from_flag(Some("1"), &url).unwrap().unwrap();
    let email = format!("User-{}@Example.test", uuid::Uuid::new_v4().simple());
    m.send_verification(&email, "first").await.unwrap();
    m.send_verification(&email, "second").await.unwrap();
    let client = redis::Client::open(url).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let key = format!("dev:mail:verify:{}", email.to_lowercase());
    assert_eq!(DevMailCapture::key("verify", &email), key);
    let v: String = redis::AsyncCommands::get(&mut conn, &key).await.unwrap();
    assert_eq!(v, "second");
    let ttl: i64 = redis::AsyncCommands::ttl(&mut conn, &key).await.unwrap();
    assert!(ttl > 3500 && ttl <= 3600);
}
