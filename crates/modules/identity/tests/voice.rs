#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use warptalk_config::JwtConfig;
use warptalk_identity::application::voice::VoiceStatePublisher;
use warptalk_identity::{
    IdentityDeps, IdentityModule, MIGRATIONS, Policy, RedisVoiceStatePublisher, VoiceCloner,
    VoiceDeps, VoiceProviderAdmin, consent_key,
};
use warptalk_kernel::{AppError, AppResult, FixedClock, UserId};
use warptalk_web::JwtKeys;

#[derive(Default)]
struct FakeProvider {
    cloned: Mutex<Vec<String>>,
    deleted: Mutex<Vec<String>>,
    fail_delete: Mutex<bool>,
}

#[async_trait::async_trait]
impl VoiceCloner for FakeProvider {
    fn provider(&self) -> &'static str {
        "fake"
    }
    async fn clone_voice(&self, wav: &[u8], _name: &str, _lang: &str) -> AppResult<String> {
        assert_eq!(&wav[0..4], b"RIFF");
        let id = format!("voice-{}", uuid::Uuid::new_v4().simple());
        self.cloned.lock().unwrap().push(id.clone());
        Ok(id)
    }
}

#[async_trait::async_trait]
impl VoiceProviderAdmin for FakeProvider {
    async fn delete_voice(&self, id: &str) -> AppResult<()> {
        if *self.fail_delete.lock().unwrap() {
            return Err(AppError::unavailable("provider down"));
        }
        self.deleted.lock().unwrap().push(id.to_owned());
        Ok(())
    }
}

struct Env {
    app: Router,
    provider: Arc<FakeProvider>,
    pool: sqlx::PgPool,
    redis: redis::aio::MultiplexedConnection,
    module: IdentityModule,
}

async fn setup() -> Option<Env> {
    let (Ok(url), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        eprintln!("DATABASE_URL/REDIS_URL not set; skipping integration test");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_db::OUTBOX_MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let client = redis::Client::open(redis_url).unwrap();
    let manager = redis::aio::ConnectionManager::new(client.clone())
        .await
        .unwrap();
    let redis = client.get_multiplexed_async_connection().await.unwrap();
    let provider = Arc::new(FakeProvider::default());
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let publisher: Arc<dyn VoiceStatePublisher> = Arc::new(RedisVoiceStatePublisher::new(manager));
    let module = IdentityModule::new(IdentityDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(FixedClock::new(Utc::now())),
        mailer: Arc::new(warptalk_identity::LogMailer),
        policy: Policy::default(),
        voice: VoiceDeps {
            admin: provider.clone(),
            cloner: provider.clone(),
            publisher,
        },
    });
    Some(Env {
        app: module.router(),
        provider,
        pool,
        redis,
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Registers a user straight in the DB-backed flow and returns (user id, access token).
async fn user(env: &Env) -> (UserId, String) {
    let email = format!("voice-{}@example.test", uuid::Uuid::new_v4().simple());
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/register",
        Some(json!({"email": email, "password": "S3cret-pass!", "fullName": "Voice User"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let id = UserId::from_uuid(b["userId"].as_str().unwrap().parse().unwrap());
    sqlx::query("UPDATE auth.users SET email_verified = true WHERE id = $1")
        .bind(id.as_uuid())
        .execute(&env.pool)
        .await
        .unwrap();
    let (s, b) = call(
        &env.app,
        "POST",
        "/auth/login",
        Some(json!({"email": email, "password": "S3cret-pass!"})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    (id, b["accessToken"].as_str().unwrap().to_owned())
}

fn consent_body() -> Value {
    json!({
        "granted": true,
        "consentVersion": "2026-09-29.v2",
        "acknowledgementHash": Sha256::digest(b"consent text")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    })
}

fn wav_b64(secs: f32, amp: f32) -> String {
    let rate = 16_000u32;
    let n = (rate as f32 * secs) as usize;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let env = 0.6 + 0.4 * (i as f32 / 4000.0).sin();
        let v = (amp * env * (i as f32 * 0.07).sin()).clamp(-1.0, 1.0);
        pcm.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    b.extend_from_slice(&pcm);
    base64::engine::general_purpose::STANDARD.encode(b)
}

async fn redis_state(env: &Env, id: UserId) -> Option<Value> {
    let mut c = env.redis.clone();
    let v: Option<String> = redis::AsyncCommands::get(&mut c, consent_key(id))
        .await
        .unwrap();
    v.map(|s| serde_json::from_str(&s).unwrap())
}

#[tokio::test]
async fn consent_grant_status_and_revoke() {
    let Some(env) = setup().await else { return };
    let (id, tok) = user(&env).await;

    let (s, b) = call(&env.app, "GET", "/me/voice", None, Some(&tok)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["consent"]["granted"], false);
    assert!(!env.module.queries().voice_clone_consent(id).await.unwrap());

    // granted:false and a bad hash are rejected
    let mut bad = consent_body();
    bad["granted"] = json!(false);
    let (s, _) = call(&env.app, "POST", "/me/voice/consent", Some(bad), Some(&tok)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let mut bad = consent_body();
    bad["acknowledgementHash"] = json!("nothex");
    let (s, _) = call(&env.app, "POST", "/me/voice/consent", Some(bad), Some(&tok)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, b) = call(
        &env.app,
        "POST",
        "/me/voice/consent",
        Some(consent_body()),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["consent"]["granted"], true);
    assert_eq!(b["consent"]["consentVersion"], "2026-09-29.v2");
    assert!(env.module.queries().voice_clone_consent(id).await.unwrap());
    assert_eq!(redis_state(&env, id).await.unwrap()["consent"], true);

    // idempotent, still one active row
    let (s, _) = call(
        &env.app,
        "POST",
        "/me/voice/consent",
        Some(consent_body()),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM voice.voice_consents WHERE user_id = $1 AND consent_status = 'GRANTED'",
    )
    .bind(id.as_uuid())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    let (s, b) = call(&env.app, "DELETE", "/me/voice/consent", None, Some(&tok)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["consent"]["granted"], false);
    assert!(!env.module.queries().voice_clone_consent(id).await.unwrap());
    assert_eq!(redis_state(&env, id).await.unwrap()["consent"], false);
    // audit row kept, now REVOKED
    let st: String =
        sqlx::query_scalar("SELECT consent_status FROM voice.voice_consents WHERE user_id = $1")
            .bind(id.as_uuid())
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(st, "REVOKED");

    // requires auth
    let (s, _) = call(&env.app, "GET", "/me/voice", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn enroll_requires_consent() {
    let Some(env) = setup().await else { return };
    let (_, tok) = user(&env).await;
    let (s, b) = call(
        &env.app,
        "POST",
        "/me/voice/enroll",
        Some(json!({"sampleBase64": wav_b64(6.0, 0.3)})),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["code"], "VOICE_CONSENT_REQUIRED");
    assert!(env.provider.cloned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn enroll_quality_checks_reject_bad_samples() {
    let Some(env) = setup().await else { return };
    let (_, tok) = user(&env).await;
    let (s, _) = call(
        &env.app,
        "POST",
        "/me/voice/consent",
        Some(consent_body()),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let cases = [
        (wav_b64(6.0, 0.0), "VOICE_SAMPLE_TOO_QUIET"),
        (wav_b64(6.0, 8.0), "VOICE_SAMPLE_CLIPPED"),
        (wav_b64(2.0, 0.3), "VOICE_SAMPLE_TOO_SHORT"),
        (wav_b64(31.0, 0.3), "VOICE_SAMPLE_TOO_LONG"),
        (
            base64::engine::general_purpose::STANDARD.encode(b"not audio at all"),
            "VOICE_SAMPLE_INVALID",
        ),
    ];
    for (sample, code) in cases {
        let (s, b) = call(
            &env.app,
            "POST",
            "/me/voice/enroll",
            Some(json!({"sampleBase64": sample})),
            Some(&tok),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{code}: {b}");
        assert_eq!(b["code"], code);
    }
    assert!(env.provider.cloned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn enroll_then_revoke_deletes_samples_and_provider_voice() {
    let Some(env) = setup().await else { return };
    let (id, tok) = user(&env).await;
    call(
        &env.app,
        "POST",
        "/me/voice/consent",
        Some(consent_body()),
        Some(&tok),
    )
    .await;
    let (s, b) = call(
        &env.app,
        "POST",
        "/me/voice/enroll",
        Some(json!({"sampleBase64": wav_b64(6.0, 0.3), "language": "en", "displayName": "Me"})),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(b["qualityScore"].as_i64().unwrap() > 0);
    let voice_id = env.provider.cloned.lock().unwrap()[0].clone();
    assert_eq!(
        env.module.queries().provider_voice(id).await.unwrap(),
        Some(voice_id.clone())
    );
    let st = redis_state(&env, id).await.unwrap();
    assert_eq!(st["consent"], true);
    assert_eq!(st["voiceId"], voice_id.as_str());
    let (_, b) = call(&env.app, "GET", "/me/voice", None, Some(&tok)).await;
    assert_eq!(b["cloneEnabled"], true);
    assert_eq!(b["profile"]["status"], "active");

    // a second enrollment replaces the first and deletes the old provider voice
    let (s, _) = call(
        &env.app,
        "POST",
        "/me/voice/enroll",
        Some(json!({"sampleBase64": wav_b64(7.0, 0.3)})),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(*env.provider.deleted.lock().unwrap(), vec![voice_id]);
    let current = env.provider.cloned.lock().unwrap()[1].clone();

    // provider is down during revoke: revoke still succeeds, deletion stays queued
    *env.provider.fail_delete.lock().unwrap() = true;
    let (s, b) = call(&env.app, "DELETE", "/me/voice/consent", None, Some(&tok)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["cloneEnabled"], false);
    assert!(b["profile"].is_null());
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM voice.voice_samples s JOIN voice.voice_profiles p \
         ON p.id = s.voice_profile_id WHERE p.user_id = $1 AND s.deleted_at IS NULL",
    )
    .bind(id.as_uuid())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
    assert_eq!(env.module.queries().provider_voice(id).await.unwrap(), None);
    let st = redis_state(&env, id).await.unwrap();
    assert_eq!(st["consent"], false);
    assert!(st.get("voiceId").is_none());
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM voice.provider_voice_deletions \
         WHERE user_id = $1 AND completed_at IS NULL",
    )
    .bind(id.as_uuid())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);

    // retry once the provider is back
    *env.provider.fail_delete.lock().unwrap() = false;
    assert!(env.module.retry_voice_deletions(100).await.unwrap() >= 1);
    assert!(env.provider.deleted.lock().unwrap().contains(&current));

    // enrolling again needs fresh consent
    let (s, b) = call(
        &env.app,
        "POST",
        "/me/voice/enroll",
        Some(json!({"sampleBase64": wav_b64(6.0, 0.3)})),
        Some(&tok),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["code"], "VOICE_CONSENT_REQUIRED");
}
