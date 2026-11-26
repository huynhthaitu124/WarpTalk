#![allow(clippy::unwrap_used)]
//! Password reset / change, verification resend (BR-87, BR-90, BR-06) and Google sign-in
//! (BR-04, BR-05) against the local Postgres. Skips when `DATABASE_URL` is unset.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tower::ServiceExt;
use warptalk_config::JwtConfig;
use warptalk_identity::{
    GoogleIdVerifier, GoogleProfile, IdentityDeps, IdentityExtras, IdentityModule, MIGRATIONS,
    MemoryThrottle, Policy, VerificationMailer,
};
use warptalk_kernel::{AppError, FixedClock};
use warptalk_web::JwtKeys;

#[derive(Default)]
struct Capture(Mutex<Vec<(&'static str, String, String)>>);

#[async_trait::async_trait]
impl VerificationMailer for Capture {
    async fn send_verification(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push(("verify", email.to_owned(), token.to_owned()));
        Ok(())
    }
    async fn send_password_reset(&self, email: &str, token: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .push(("reset", email.to_owned(), token.to_owned()));
        Ok(())
    }
}

impl Capture {
    fn all(&self, kind: &str, email: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, e, _)| *k == kind && e == email)
            .map(|(_, _, t)| t.clone())
            .collect()
    }
    fn last(&self, kind: &str, email: &str) -> String {
        self.all(kind, email).pop().unwrap()
    }
}

/// `idToken` format: `sub|email|name`; anything else is rejected like a bad signature.
struct StubGoogle;

#[async_trait::async_trait]
impl GoogleIdVerifier for StubGoogle {
    async fn verify(&self, id_token: &str) -> Result<GoogleProfile, AppError> {
        let p: Vec<&str> = id_token.split('|').collect();
        if p.len() != 3 {
            return Err(AppError::coded(
                401,
                "INVALID_GOOGLE_TOKEN",
                "invalid google id token",
            ));
        }
        Ok(GoogleProfile {
            sub: p[0].to_owned(),
            email: p[1].to_owned(),
            name: Some(p[2].to_owned()).filter(|n| !n.is_empty()),
            picture: None,
        })
    }
}

struct Env {
    app: Router,
    clock: Arc<FixedClock>,
    mail: Arc<Capture>,
}

async fn setup(with_google: bool) -> Option<Env> {
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
    let mail = Arc::new(Capture::default());
    let module = IdentityModule::with_extras(
        IdentityDeps {
            pool,
            jwt,
            clock: clock.clone(),
            mailer: mail.clone(),
            policy: Policy::default(),
            voice: warptalk_identity::VoiceDeps::default(),
        },
        IdentityExtras {
            throttle: Some(Arc::new(MemoryThrottle::new(clock.clone()))),
            google: with_google.then(|| Arc::new(StubGoogle) as Arc<dyn GoogleIdVerifier>),
        },
    );
    Some(Env {
        app: module.router(),
        clock,
        mail,
    })
}

async fn call(
    env: &Env,
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
    let resp = env.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn fresh_email() -> String {
    format!("user-{}@example.test", uuid::Uuid::new_v4().simple())
}

fn fresh_sub() -> String {
    format!("g-{}", uuid::Uuid::new_v4().simple())
}

const PW: &str = "S3cret-pass!";
const NEW_PW: &str = "An0ther-pass!";

async fn register(env: &Env, email: &str) {
    let (s, b) = call(
        env,
        "POST",
        "/auth/register",
        Some(json!({"email": email, "password": PW, "fullName": "Test User"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
}

async fn register_verified(env: &Env) -> String {
    let email = fresh_email();
    register(env, &email).await;
    let t = env.mail.last("verify", &email);
    let (s, _) = call(
        env,
        "POST",
        "/auth/verify-email",
        Some(json!({"token": t})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    email
}

async fn login(env: &Env, email: &str, pw: &str) -> (StatusCode, Value) {
    call(
        env,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": pw})),
        None,
    )
    .await
}

async fn login_ok(env: &Env, email: &str, pw: &str) -> Value {
    let (s, b) = login(env, email, pw).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b
}

async fn refresh(env: &Env, rt: &Value) -> StatusCode {
    call(
        env,
        "POST",
        "/auth/refresh",
        Some(json!({"refreshToken": rt})),
        None,
    )
    .await
    .0
}

async fn forgot(env: &Env, email: &str) -> StatusCode {
    call(
        env,
        "POST",
        "/auth/forgot-password",
        Some(json!({"email": email})),
        None,
    )
    .await
    .0
}

async fn reset(env: &Env, token: &str, pw: &str) -> (StatusCode, Value) {
    call(
        env,
        "POST",
        "/auth/reset-password",
        Some(json!({"token": token, "newPassword": pw})),
        None,
    )
    .await
}

fn code(b: &Value) -> &str {
    b["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn reset_flow_single_use_latest_only_and_revokes_all_sessions() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = register_verified(&env).await;
    let s1 = login_ok(&env, &email, PW).await;
    let s2 = login_ok(&env, &email, PW).await;

    assert_eq!(forgot(&env, &email).await, StatusCode::ACCEPTED);
    let t1 = env.mail.last("reset", &email);
    env.clock.set(env.clock_now() + Duration::seconds(61));
    assert_eq!(
        forgot(&env, &email.to_uppercase()).await,
        StatusCode::ACCEPTED
    );
    let t2 = env.mail.last("reset", &email);
    assert_ne!(t1, t2);

    // Latest-only: the first token no longer works.
    let (s, b) = reset(&env, &t1, NEW_PW).await;
    assert_eq!((s, code(&b)), (StatusCode::BAD_REQUEST, "INVALID_TOKEN"));

    // Policy violation does not burn the token.
    let (s, b) = reset(&env, &t2, "short").await;
    assert_eq!((s, code(&b)), (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"));

    let (s, _) = reset(&env, &t2, NEW_PW).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // Single-use.
    let (s, b) = reset(&env, &t2, "Yet-an0ther-pass").await;
    assert_eq!((s, code(&b)), (StatusCode::BAD_REQUEST, "INVALID_TOKEN"));

    assert_eq!(login(&env, &email, PW).await.0, StatusCode::UNAUTHORIZED);
    login_ok(&env, &email, NEW_PW).await;
    // BR-06: every pre-existing session is gone.
    assert_eq!(
        refresh(&env, &s1["refreshToken"]).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        refresh(&env, &s2["refreshToken"]).await,
        StatusCode::UNAUTHORIZED
    );
}

impl Env {
    fn clock_now(&self) -> chrono::DateTime<Utc> {
        use warptalk_kernel::Clock;
        self.clock.now()
    }
}

#[tokio::test]
async fn reset_token_expires_after_30_minutes() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = register_verified(&env).await;
    forgot(&env, &email).await;
    let t = env.mail.last("reset", &email);
    env.clock.set(env.clock_now() + Duration::minutes(31));
    let (s, b) = reset(&env, &t, NEW_PW).await;
    assert_eq!((s, code(&b)), (StatusCode::BAD_REQUEST, "INVALID_TOKEN"));
    login_ok(&env, &email, PW).await;
}

#[tokio::test]
async fn forgot_password_never_reveals_accounts_and_validates_shape() {
    let Some(env) = setup(false).await else {
        return;
    };
    let ghost = fresh_email();
    assert_eq!(forgot(&env, &ghost).await, StatusCode::ACCEPTED);
    assert!(env.mail.all("reset", &ghost).is_empty());
    assert_eq!(forgot(&env, "not-an-email").await, StatusCode::BAD_REQUEST);
    let (s, _) = reset(&env, "bogus", NEW_PW).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_mail_cooldown_and_window_limits() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = register_verified(&env).await;
    for _ in 0..3 {
        assert_eq!(forgot(&env, &email).await, StatusCode::ACCEPTED);
    }
    // 60 s cooldown: only the first request mailed a token.
    assert_eq!(env.mail.all("reset", &email).len(), 1);
    // 5 per 15 min: sends at +61 s steps are allowed until the cap.
    for _ in 0..6 {
        env.clock.set(env.clock_now() + Duration::seconds(61));
        forgot(&env, &email).await;
    }
    assert_eq!(env.mail.all("reset", &email).len(), 5);
    // After the window rolls over, sending works again.
    env.clock.set(env.clock_now() + Duration::minutes(16));
    forgot(&env, &email).await;
    assert_eq!(env.mail.all("reset", &email).len(), 6);
}

#[tokio::test]
async fn resend_verification_rules() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = fresh_email();
    register(&env, &email).await;
    let first = env.mail.last("verify", &email);

    let resend = |e: String| {
        let env = &env;
        async move {
            call(
                env,
                "POST",
                "/auth/resend-verification",
                Some(json!({"email": e})),
                None,
            )
            .await
            .0
        }
    };
    // Registration's own mail does not arm the cooldown; the first resend is sent.
    assert_eq!(resend(email.clone()).await, StatusCode::ACCEPTED);
    let second = env.mail.last("verify", &email);
    assert_ne!(first, second);
    // Cooldown: immediate repeat is accepted silently, nothing sent.
    assert_eq!(resend(email.clone()).await, StatusCode::ACCEPTED);
    assert_eq!(env.mail.all("verify", &email).len(), 2);

    // BR-90: the older token no longer verifies; the newest does, once.
    let post = |t: String| {
        let env = &env;
        async move {
            call(
                env,
                "POST",
                "/auth/verify-email",
                Some(json!({"token": t})),
                None,
            )
            .await
            .0
        }
    };
    assert_eq!(post(first).await, StatusCode::BAD_REQUEST);
    assert_eq!(post(second.clone()).await, StatusCode::NO_CONTENT);
    assert_eq!(post(second).await, StatusCode::BAD_REQUEST);

    // Verified or unknown accounts: 202 and no mail.
    env.clock.set(env.clock_now() + Duration::seconds(61));
    assert_eq!(resend(email.clone()).await, StatusCode::ACCEPTED);
    assert_eq!(env.mail.all("verify", &email).len(), 2);
    let ghost = fresh_email();
    assert_eq!(resend(ghost.clone()).await, StatusCode::ACCEPTED);
    assert!(env.mail.all("verify", &ghost).is_empty());
    assert_eq!(resend("bad".into()).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn change_password_revokes_other_sessions_only() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = register_verified(&env).await;
    let a = login_ok(&env, &email, PW).await;
    let b = login_ok(&env, &email, PW).await;
    let at = a["accessToken"].as_str().unwrap();

    let (s, body) = call(
        &env,
        "POST",
        "/me/password",
        Some(json!({"currentPassword": "wrong-pass-1", "newPassword": NEW_PW})),
        Some(at),
    )
    .await;
    assert_eq!(
        (s, code(&body)),
        (StatusCode::BAD_REQUEST, "INVALID_CURRENT_PASSWORD")
    );
    let (s, _) = call(
        &env,
        "POST",
        "/me/password",
        Some(json!({"currentPassword": PW, "newPassword": "short"})),
        Some(at),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &env,
        "POST",
        "/me/password",
        Some(json!({"currentPassword": PW, "newPassword": NEW_PW})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, _) = call(
        &env,
        "POST",
        "/me/password",
        Some(json!({"currentPassword": PW, "newPassword": NEW_PW})),
        Some(at),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(login(&env, &email, PW).await.0, StatusCode::UNAUTHORIZED);
    login_ok(&env, &email, NEW_PW).await;
    assert_eq!(
        refresh(&env, &b["refreshToken"]).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(refresh(&env, &a["refreshToken"]).await, StatusCode::OK);
}

#[tokio::test]
async fn change_password_wrong_attempts_are_rate_limited() {
    let Some(env) = setup(false).await else {
        return;
    };
    let email = register_verified(&env).await;
    let a = login_ok(&env, &email, PW).await;
    let at = a["accessToken"].as_str().unwrap();
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        last = call(
            &env,
            "POST",
            "/me/password",
            Some(json!({"currentPassword": "wrong-pass-1", "newPassword": NEW_PW})),
            Some(at),
        )
        .await
        .0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

async fn google(env: &Env, token: &str) -> (StatusCode, Value) {
    call(
        env,
        "POST",
        "/auth/google",
        Some(json!({"idToken": token})),
        None,
    )
    .await
}

#[tokio::test]
async fn google_not_configured_is_503() {
    let Some(env) = setup(false).await else {
        return;
    };
    let (s, b) = google(&env, "x|y@z.test|n").await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::SERVICE_UNAVAILABLE, "GOOGLE_NOT_CONFIGURED")
    );
}

#[tokio::test]
async fn google_invalid_token_is_401() {
    let Some(env) = setup(true).await else { return };
    let (s, b) = google(&env, "garbage").await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::UNAUTHORIZED, "INVALID_GOOGLE_TOKEN")
    );
}

#[tokio::test]
async fn google_creates_verified_passwordless_user_and_logs_in_again() {
    let Some(env) = setup(true).await else { return };
    let (sub, email) = (fresh_sub(), fresh_email());
    let (s, b) = google(&env, &format!("{sub}|{}|Gina G", email.to_uppercase())).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["email"], email);
    for k in [
        "userId",
        "accessToken",
        "accessTokenExpiresAt",
        "refreshToken",
    ] {
        assert!(b.get(k).is_some(), "missing {k}");
    }
    let at = b["accessToken"].as_str().unwrap();
    let (_, me) = call(&env, "GET", "/me", None, Some(at)).await;
    assert_eq!(me["emailVerified"], true);
    assert_eq!(me["googleLinked"], true);
    assert_eq!(me["hasPassword"], false);
    assert_eq!(me["fullName"], "Gina G");

    // Same Google identity again -> same account (even if Google's email text changed).
    let (s, b2) = google(&env, &format!("{sub}|{email}|Gina")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b2["userId"], b["userId"]);
    // No password: local login fails generically.
    assert_eq!(login(&env, &email, PW).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh(&env, &b["refreshToken"]).await, StatusCode::OK);

    // Unlink is forbidden without a local password (BR-05) ...
    let (s, e) = call(&env, "DELETE", "/me/google/link", None, Some(at)).await;
    assert_eq!(
        (s, code(&e)),
        (StatusCode::CONFLICT, "GOOGLE_UNLINK_REQUIRES_PASSWORD")
    );
    // ... and possible once a password was set through the reset flow.
    forgot(&env, &email).await;
    let t = env.mail.last("reset", &email);
    assert_eq!(reset(&env, &t, NEW_PW).await.0, StatusCode::NO_CONTENT);
    let fresh = login_ok(&env, &email, NEW_PW).await;
    let at2 = fresh["accessToken"].as_str().unwrap();
    let (s, _) = call(&env, "DELETE", "/me/google/link", None, Some(at2)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, me) = call(&env, "GET", "/me", None, Some(at2)).await;
    assert_eq!(
        (me["googleLinked"].clone(), me["hasPassword"].clone()),
        (json!(false), json!(true))
    );
    // Idempotent.
    let (s, _) = call(&env, "DELETE", "/me/google/link", None, Some(at2)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn google_auto_links_only_verified_local_accounts() {
    let Some(env) = setup(true).await else { return };

    // Verified local account: linked case-insensitively, same user.
    let email = register_verified(&env).await;
    let local = login_ok(&env, &email, PW).await;
    let sub = fresh_sub();
    let (s, g) = google(&env, &format!("{sub}|{}|Someone", email.to_uppercase())).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["userId"], local["userId"]);
    // A different Google account for the already-linked local account is refused.
    let (s, b) = google(&env, &format!("{}|{email}|Other", fresh_sub())).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, "GOOGLE_ACCOUNT_MISMATCH")
    );

    // Unverified local account: blocked (BR-04), verification mail re-sent, no session.
    let pending = fresh_email();
    register(&env, &pending).await;
    let before = env.mail.all("verify", &pending).len();
    let (s, b) = google(&env, &format!("{}|{pending}|Hijacker", fresh_sub())).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, "GOOGLE_LINK_REQUIRES_VERIFIED_EMAIL")
    );
    assert_eq!(env.mail.all("verify", &pending).len(), before + 1);
    // The local password account is still the only way in.
    assert_eq!(login(&env, &pending, PW).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn manual_link_requires_same_email_and_unique_google_account() {
    let Some(env) = setup(true).await else { return };
    let email = register_verified(&env).await;
    let at = login_ok(&env, &email, PW).await["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let link = |tok: String| {
        let (env, at) = (&env, at.clone());
        async move {
            call(
                env,
                "POST",
                "/me/google/link",
                Some(json!({"idToken": tok})),
                Some(&at),
            )
            .await
        }
    };
    let sub = fresh_sub();

    let (s, b) = link(format!("{sub}|{}|X", fresh_email())).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, "GOOGLE_EMAIL_MISMATCH")
    );
    let (s, b) = link("junk".into()).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::UNAUTHORIZED, "INVALID_GOOGLE_TOKEN")
    );

    let (s, _) = link(format!("{sub}|{}|X", email.to_uppercase())).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = link(format!("{sub}|{email}|X")).await; // idempotent
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, b) = link(format!("{}|{email}|X", fresh_sub())).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, "GOOGLE_ALREADY_LINKED")
    );

    // The Google account is bound to one WarpTalk account only.
    let other = register_verified(&env).await;
    let at2 = login_ok(&env, &other, PW).await["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    // Same `sub` presented with the other account's email: email check passes, sub is taken.
    let (s, b) = call(
        &env,
        "POST",
        "/me/google/link",
        Some(json!({"idToken": format!("{sub}|{other}|X")})),
        Some(&at2),
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, "GOOGLE_ACCOUNT_IN_USE")
    );

    // Signing in with Google now reaches the linked account.
    let (s, g) = google(&env, &format!("{sub}|{email}|X")).await;
    assert_eq!(s, StatusCode::OK);
    let (_, me) = call(&env, "GET", "/me", None, g["accessToken"].as_str()).await;
    assert_eq!(me["email"], email);
}
