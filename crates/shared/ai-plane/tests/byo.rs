#![allow(clippy::unwrap_used)]

//! BYO-LLM: resolver precedence, fallback policy, cross-instance cache invalidation (Redis) and
//! the assistant REST surface with a wiremock-backed key validation. Needs DATABASE_URL and
//! REDIS_URL; skips otherwise.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_ai_plane::ai::{AiProviders, PlatformKey, PlatformKeys};
use warptalk_ai_plane::{
    BaseUrlPolicy, ByoDeps, KeyVault, LlmKeyValidator, RedisConfigNotifier, StaticExternalPolicy,
};
use warptalk_assistant::application::ports::{
    ConfigNotifier, LlmConfigRepository, NoopNotifier, SecretVault, WorkspaceAccess,
};
use warptalk_assistant::domain::byo::{
    BoundaryKind, FallbackPolicy, LlmConfigInput, LlmProviderKind, LlmPurpose,
};
use warptalk_assistant::infrastructure::PgLlmConfigRepository;
use warptalk_assistant::suggestion::{DataBoundary, LlmResolver, Purpose, ResolveError};
use warptalk_assistant::{AssistantDeps, AssistantModule, MIGRATIONS};
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_llm::ProviderId;
use warptalk_settings::SettingsStore;
use warptalk_web::JwtKeys;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEK_B64: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn env() -> Option<(String, String)> {
    match (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL")) {
        (Ok(d), Ok(r)) => Some((d, r)),
        _ => {
            println!("DATABASE_URL / REDIS_URL unset; skipping");
            None
        }
    }
}

async fn pool(url: &str) -> PgPool {
    let pool = warptalk_db::connect(url, 4).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    pool
}

fn vault() -> Arc<dyn SecretVault> {
    Arc::new(KeyVault::from_base64(KEK_B64, 1).unwrap())
}

fn platform_keys() -> PlatformKeys {
    PlatformKeys::default().with(
        ProviderId::OpenAi,
        PlatformKey {
            api_key: "platform-key-0000".into(),
            base_url: None,
        },
    )
}

fn providers(
    pool: &PgPool,
    vault: Arc<dyn SecretVault>,
    external_allowed: bool,
) -> Arc<AiProviders> {
    let settings = SettingsStore::new(pool.clone(), None);
    warptalk_settings::namespaces::register_defaults(&settings);
    AiProviders::with_byo(
        settings,
        platform_keys(),
        ByoDeps::new(
            Arc::new(PgLlmConfigRepository::new(pool.clone())),
            vault,
            Arc::new(StaticExternalPolicy(external_allowed)),
        ),
    )
}

fn input(provider: LlmProviderKind, model: &str) -> LlmConfigInput {
    LlmConfigInput {
        provider,
        model: model.into(),
        base_url: None,
        reasoning: None,
        max_output_tokens: None,
        timeout_ms: None,
        data_boundary: BoundaryKind::External,
        fallback_policy: FallbackPolicy::None,
        enabled: true,
    }
}

async fn seed_credential(
    repo: &PgLlmConfigRepository,
    vault: &Arc<dyn SecretVault>,
    ws: WorkspaceId,
    provider: LlmProviderKind,
) {
    let sealed = vault
        .seal(
            ws,
            provider,
            &secrecy::SecretString::from("sk-byo-secret-7777"),
        )
        .unwrap();
    repo.upsert_credential(ws, provider, &sealed, UserId::new(), chrono::Utc::now())
        .await
        .unwrap();
}

#[tokio::test]
async fn workspace_override_beats_platform_default() {
    let Some((db, _)) = env() else { return };
    let pool = pool(&db).await;
    let vault = vault();
    let repo = PgLlmConfigRepository::new(pool.clone());
    let ai = providers(&pool, vault.clone(), true);
    let (byo_ws, plain_ws) = (WorkspaceId::new(), WorkspaceId::new());

    seed_credential(&repo, &vault, byo_ws, LlmProviderKind::Anthropic).await;
    let mut cfg = input(LlmProviderKind::Anthropic, "claude-byo-model");
    cfg.data_boundary = BoundaryKind::WorkspaceControlled;
    let saved = repo
        .upsert_config(
            byo_ws,
            LlmPurpose::SuggestionDecide,
            &cfg,
            UserId::new(),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status.as_str(), "ok");

    let r = ai
        .for_purpose(byo_ws, Purpose::SuggestionDecide)
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("claude-byo-model"));
    assert_eq!(r.boundary, DataBoundary::WorkspaceControlled);
    assert_eq!(
        r.key_source,
        warptalk_assistant::suggestion::KeySource::Workspace
    );
    assert_eq!(r.provider.id(), ProviderId::Anthropic);

    // Another purpose of the same workspace, and another workspace: platform default.
    let r = ai
        .for_purpose(byo_ws, Purpose::SuggestionGenerate)
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("gpt-4.1"));
    assert_eq!(r.boundary, DataBoundary::External);
    let r = ai
        .for_purpose(plain_ws, Purpose::SuggestionDecide)
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("gpt-4.1"));

    // Disabled override: platform again.
    let mut off = cfg.clone();
    off.enabled = false;
    repo.upsert_config(
        byo_ws,
        LlmPurpose::SuggestionDecide,
        &off,
        UserId::new(),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    ai.invalidate_workspace(byo_ws);
    let r = ai
        .for_purpose(byo_ws, Purpose::SuggestionDecide)
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("gpt-4.1"));
}

#[tokio::test]
async fn fallback_policy_is_explicit_and_consent_bound() {
    let Some((db, _)) = env() else { return };
    let pool = pool(&db).await;
    let vault = vault();
    let repo = PgLlmConfigRepository::new(pool.clone());
    let now = chrono::Utc::now();

    // BYO configured without a credential -> status invalid. Fallback `none`: error surfaces.
    let ws = WorkspaceId::new();
    repo.upsert_config(
        ws,
        LlmPurpose::SuggestionDecide,
        &input(LlmProviderKind::OpenAi, "m"),
        UserId::new(),
        now,
    )
    .await
    .unwrap();
    let ai = providers(&pool, vault.clone(), true);
    let err = ai
        .for_purpose(ws, Purpose::SuggestionDecide)
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, ResolveError::Failed(ref m) if m.contains("unavailable")),
        "{err}"
    );

    // `platform_if_allowed` + consent: platform provider, never silently otherwise.
    let ws2 = WorkspaceId::new();
    let mut cfg = input(LlmProviderKind::OpenAi, "m");
    cfg.fallback_policy = FallbackPolicy::PlatformIfAllowed;
    repo.upsert_config(ws2, LlmPurpose::SuggestionDecide, &cfg, UserId::new(), now)
        .await
        .unwrap();
    let r = ai
        .for_purpose(ws2, Purpose::SuggestionDecide)
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("gpt-4.1"));

    // Same config but external_llm_allowed = false: no fallback, error.
    let strict = providers(&pool, vault.clone(), false);
    assert!(
        strict
            .for_purpose(ws2, Purpose::SuggestionDecide)
            .await
            .is_err()
    );

    // BYO external endpoint with consent denied: refused (and no platform fallback either).
    let ws3 = WorkspaceId::new();
    seed_credential(&repo, &vault, ws3, LlmProviderKind::OpenAi).await;
    let mut cfg = input(LlmProviderKind::OpenAi, "m");
    cfg.fallback_policy = FallbackPolicy::PlatformIfAllowed;
    repo.upsert_config(ws3, LlmPurpose::SuggestionDecide, &cfg, UserId::new(), now)
        .await
        .unwrap();
    assert!(
        strict
            .for_purpose(ws3, Purpose::SuggestionDecide)
            .await
            .is_err()
    );
    assert!(ai.for_purpose(ws3, Purpose::SuggestionDecide).await.is_ok());

    // ... but a workspace-controlled endpoint is exempt from the consent gate.
    let ws4 = WorkspaceId::new();
    seed_credential(&repo, &vault, ws4, LlmProviderKind::OpenAi).await;
    let mut cfg = input(LlmProviderKind::OpenAi, "m");
    cfg.data_boundary = BoundaryKind::WorkspaceControlled;
    repo.upsert_config(ws4, LlmPurpose::SuggestionDecide, &cfg, UserId::new(), now)
        .await
        .unwrap();
    let r = strict
        .for_purpose(ws4, Purpose::SuggestionDecide)
        .await
        .unwrap();
    assert_eq!(r.boundary, DataBoundary::WorkspaceControlled);

    // BYO disabled (no KEK) with a stored override and no fallback: clear error.
    let off = providers(
        &pool,
        Arc::new(warptalk_ai_plane::DisabledVault(
            "AI_KEYS_KEK is not set".into(),
        )),
        true,
    );
    let err = off
        .for_purpose(ws4, Purpose::SuggestionDecide)
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("disabled"), "{err}");
}

#[tokio::test]
async fn config_change_invalidates_other_resolver_instances_via_redis() {
    let Some((db, redis_url)) = env() else { return };
    let pool = pool(&db).await;
    let vault = vault();
    let repo = PgLlmConfigRepository::new(pool.clone());
    let a = providers(&pool, vault.clone(), true);
    let b = providers(&pool, vault.clone(), true);
    let _la = a.spawn_invalidation_listener(&redis_url).unwrap();
    let _lb = b.spawn_invalidation_listener(&redis_url).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let ws = WorkspaceId::new();
    // Prime both caches with "no override" (cache TTL is 60 s, far beyond this test).
    for ai in [&a, &b] {
        let r = ai.for_purpose(ws, Purpose::SuggestionDecide).await.unwrap();
        assert_eq!(r.model.as_deref(), Some("gpt-4.1"));
    }

    seed_credential(&repo, &vault, ws, LlmProviderKind::Gemini).await;
    repo.upsert_config(
        ws,
        LlmPurpose::SuggestionDecide,
        &input(LlmProviderKind::Gemini, "gemini-byo"),
        UserId::new(),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    RedisConfigNotifier::new(redis).changed(ws).await;

    for (name, ai) in [("a", &a), ("b", &b)] {
        let mut seen = None;
        for _ in 0..40 {
            let r = ai.for_purpose(ws, Purpose::SuggestionDecide).await.unwrap();
            if r.model.as_deref() == Some("gemini-byo") {
                seen = Some(r);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let r = seen.unwrap_or_else(|| panic!("instance {name} kept the stale cache"));
        assert_eq!(r.provider.id(), ProviderId::Gemini);
    }
}

// ---- REST -------------------------------------------------------------------------------

struct Roles(HashMap<(Uuid, Uuid), WorkspaceRole>);

#[async_trait]
impl WorkspaceAccess for Roles {
    async fn member_role(&self, ws: WorkspaceId, user: UserId) -> AppResult<Option<WorkspaceRole>> {
        Ok(self.0.get(&(ws.as_uuid(), user.as_uuid())).copied())
    }
}

struct Rest {
    app: Router,
    jwt: JwtKeys,
    pool: PgPool,
    ws: WorkspaceId,
    owner: String,
    admin: String,
    member: String,
    outsider: String,
}

fn token(jwt: &JwtKeys, user: UserId) -> String {
    jwt.issue_access(user, Uuid::new_v4(), false, chrono::Utc::now())
        .unwrap()
        .0
}

async fn rest(pool: PgPool, notifier: Arc<dyn ConfigNotifier>) -> Rest {
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".into(),
        issuer: "t".into(),
        audience: "t".into(),
        access_ttl_seconds: 600,
        refresh_ttl_seconds: 600,
    });
    let ws = WorkspaceId::new();
    let (owner, admin, member, outsider) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let roles = Roles(HashMap::from([
        ((ws.as_uuid(), owner.as_uuid()), WorkspaceRole::Owner),
        ((ws.as_uuid(), admin.as_uuid()), WorkspaceRole::Admin),
        ((ws.as_uuid(), member.as_uuid()), WorkspaceRole::Member),
    ]));
    let module = AssistantModule::new(AssistantDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        access: Arc::new(roles),
        vault: vault(),
        validator: Arc::new(LlmKeyValidator::new(BaseUrlPolicy::AllowInsecure)),
        notifier,
    });
    let app = Router::new().nest("/api/v1", module.router());
    Rest {
        app,
        owner: token(&jwt, owner),
        admin: token(&jwt, admin),
        member: token(&jwt, member),
        outsider: token(&jwt, outsider),
        jwt,
        pool,
        ws,
    }
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: &str,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json");
    let req = match body {
        Some(b) => req.body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

async fn credential_rows(pool: &PgPool, ws: WorkspaceId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM assistant.workspace_llm_credentials WHERE workspace_id = $1",
    )
    .bind(ws.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn rest_requires_owner_or_admin() {
    let Some((db, _)) = env() else { return };
    let r = rest(pool(&db).await, Arc::new(NoopNotifier)).await;
    let cfg = format!("/api/v1/ai/workspaces/{}/config", r.ws);
    let cred = format!("/api/v1/ai/workspaces/{}/credentials/openai", r.ws);
    let body = json!({"purposes": {"assistant": {"provider": "openai", "model": "gpt-x"}}});

    for tok in [&r.member, &r.outsider] {
        let (s, v, _) = call(&r.app, "GET", &cfg, None, tok).await;
        assert_eq!(
            (s, v["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("FORBIDDEN"))
        );
        assert_eq!(
            call(&r.app, "PUT", &cfg, Some(body.clone()), tok).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &r.app,
                "PUT",
                &cred,
                Some(json!({"apiKey": "sk-whatever-1234"})),
                tok
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&r.app, "DELETE", &cred, None, tok).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&r.app, "POST", &format!("{cred}/test"), None, tok)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for tok in [&r.owner, &r.admin] {
        assert_eq!(call(&r.app, "GET", &cfg, None, tok).await.0, StatusCode::OK);
    }
    // Unauthenticated: 401 from the extractor.
    let resp = r
        .app
        .clone()
        .oneshot(Request::builder().uri(&cfg).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(credential_rows(&r.pool, r.ws).await, 0);
    let _ = &r.jwt;
}

#[tokio::test]
async fn rest_credential_flow_validates_before_storing_and_never_echoes_the_key() {
    let Some((db, _)) = env() else { return };
    let pool = pool(&db).await;
    let r = rest(pool.clone(), Arc::new(NoopNotifier)).await;
    let mock = MockServer::start().await;
    let good = "sk-live-GOODKEY-abcd1234";
    let bad = "sk-live-BADKEY-wxyz9999";
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", format!("Bearer {good}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", format!("Bearer {bad}").as_str()))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": "Incorrect API key provided"}})),
        )
        .mount(&mock)
        .await;
    let ws = r.ws;
    let cfg = format!("/api/v1/ai/workspaces/{ws}/config");
    let cred = format!("/api/v1/ai/workspaces/{ws}/credentials/openai_compatible");

    // Config first: no credential yet -> status invalid.
    let (s, v, _) = call(
        &r.app,
        "PUT",
        &cfg,
        Some(json!({
            "baseUrl": mock.uri(),
            "purposes": {
                "assistant": {"provider": "openai_compatible", "model": "gpt-x"},
                "suggestion_decide": {"provider": "openai_compatible", "model": "gpt-mini",
                                      "fallbackPolicy": "platform_if_allowed"}
            }
        })),
        &r.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["purposes"]["assistant"]["status"], "invalid");
    assert_eq!(
        v["purposes"]["suggestion_decide"]["fallbackPolicy"],
        "platform_if_allowed"
    );
    assert_eq!(v["reindexRequired"], false);

    // Bad key: 400 INVALID_API_KEY, nothing stored.
    let (s, v, text) = call(&r.app, "PUT", &cred, Some(json!({"apiKey": bad})), &r.admin).await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("INVALID_API_KEY")),
        "{text}"
    );
    assert!(!text.contains("BADKEY"));
    assert_eq!(credential_rows(&pool, ws).await, 0);

    // Good key: stored, only last4 comes back.
    let (s, v, text) = call(
        &r.app,
        "PUT",
        &cred,
        Some(json!({"apiKey": good})),
        &r.owner,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(v["last4"], "1234");
    assert_eq!(v["provider"], "openai_compatible");
    assert!(!text.contains("GOODKEY"));
    assert_eq!(credential_rows(&pool, ws).await, 1);

    // Stored as ciphertext only.
    let (ct, wrapped, last4): (Vec<u8>, Vec<u8>, String) = sqlx::query_as(
        "SELECT ciphertext, wrapped_dek, last4 FROM assistant.workspace_llm_credentials WHERE workspace_id = $1",
    )
    .bind(ws.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    for blob in [&ct, &wrapped] {
        assert!(!blob.windows(6).any(|w| w == b"GOODKE"));
    }
    assert_eq!(last4, "1234");

    // GET shows status ok and the hint, never the key.
    let (s, v, text) = call(&r.app, "GET", &cfg, None, &r.owner).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["purposes"]["assistant"]["status"], "ok");
    assert_eq!(v["credentials"][0]["last4"], "1234");
    assert!(!text.contains("GOODKEY") && !text.contains(good));

    // The resolver now builds a provider from the stored key and really authenticates with it.
    let ai = providers(&pool, vault(), true);
    let resolved = ai.llm_for_purpose(ws, LlmPurpose::Assistant).await.unwrap();
    assert_eq!(resolved.model.as_deref(), Some("gpt-x"));
    resolved.provider.validate().await.unwrap();

    // Test endpoint.
    let (s, v, _) = call(&r.app, "POST", &format!("{cred}/test"), None, &r.owner).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");

    // Rotating to a rejected key leaves the stored one untouched.
    let (s, _, _) = call(&r.app, "PUT", &cred, Some(json!({"apiKey": bad})), &r.owner).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v, _) = call(&r.app, "GET", &cfg, None, &r.owner).await;
    assert_eq!(
        (s, v["credentials"][0]["last4"].as_str()),
        (StatusCode::OK, Some("1234"))
    );

    // Delete: configs fall to invalid.
    let (s, _, _) = call(&r.app, "DELETE", &cred, None, &r.owner).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(credential_rows(&pool, ws).await, 0);
    let (_, v, _) = call(&r.app, "GET", &cfg, None, &r.owner).await;
    assert_eq!(v["purposes"]["assistant"]["status"], "invalid");
    assert_eq!(v["purposes"]["assistant"]["lastErrorCode"], "NO_CREDENTIAL");
    let (s, _, _) = call(&r.app, "DELETE", &cred, None, &r.owner).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rest_validation_rules_and_embedding_reindex_flag() {
    let Some((db, _)) = env() else { return };
    let r = rest(pool(&db).await, Arc::new(NoopNotifier)).await;
    let cfg = format!("/api/v1/ai/workspaces/{}/config", r.ws);
    let put = |body: Value| {
        let (app, tok, cfg) = (r.app.clone(), r.owner.clone(), cfg.clone());
        async move { call(&app, "PUT", &cfg, Some(body), &tok).await }
    };

    let emb =
        |model: &str| json!({"purposes": {"embedding": {"provider": "openai", "model": model}}});
    let (s, v, _) = put(emb("text-embedding-3-small")).await;
    assert_eq!(
        (s, &v["reindexRequired"]),
        (StatusCode::OK, &json!(true)),
        "{v}"
    );
    let (_, v, _) = put(emb("text-embedding-3-small")).await;
    assert_eq!(v["reindexRequired"], false);
    let (_, v, _) = put(emb("text-embedding-3-large")).await;
    assert_eq!(v["reindexRequired"], true);
    // Non-embedding change does not warn.
    let (_, v, _) =
        put(json!({"purposes": {"assistant": {"provider": "openai", "model": "m"}}})).await;
    assert_eq!(v["reindexRequired"], false);
    // Removing the embedding override changes the profile too.
    let (_, v, _) = put(json!({"purposes": {"embedding": null}})).await;
    assert_eq!(v["reindexRequired"], true);
    assert!(v["purposes"].get("embedding").is_none());

    let code = |v: &Value| v["code"].as_str().map(str::to_owned);
    let (s, v, _) = put(json!({"purposes": {"nope": {"provider": "openai", "model": "m"}}})).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, Some("INVALID_PURPOSE".into()))
    );
    let (s, v, _) =
        put(json!({"purposes": {"assistant": {"provider": "cohere", "model": "m"}}})).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, Some("INVALID_PROVIDER".into()))
    );
    let (s, v, _) =
        put(json!({"purposes": {"assistant": {"provider": "openai_compatible", "model": "m"}}}))
            .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, Some("INVALID_BASE_URL".into()))
    );
    let (s, v, _) = put(json!({"purposes": {"assistant": {"provider": "openai", "model": "m", "dataBoundary": "workspace_controlled"}}})).await;
    assert_eq!(
        (s, code(&v)),
        (
            StatusCode::BAD_REQUEST,
            Some("ACKNOWLEDGEMENT_REQUIRED".into())
        )
    );
    let (s, _, _) = put(json!({"purposes": {"assistant": {"provider": "openai", "model": "m", "dataBoundary": "workspace_controlled"}}, "acknowledgeWorkspaceControlled": true})).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v, _) = call(
        &r.app,
        "PUT",
        &format!("/api/v1/ai/workspaces/{}/credentials/openai", r.ws),
        Some(json!({"apiKey": "short"})),
        &r.owner,
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, Some("INVALID_API_KEY".into()))
    );
}

#[tokio::test]
async fn rest_reports_byo_disabled_without_a_kek() {
    let Some((db, _)) = env() else { return };
    let pool = pool(&db).await;
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".into(),
        issuer: "t".into(),
        audience: "t".into(),
        access_ttl_seconds: 600,
        refresh_ttl_seconds: 600,
    });
    let (ws, owner) = (WorkspaceId::new(), UserId::new());
    let module = AssistantModule::new(AssistantDeps {
        pool,
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        access: Arc::new(Roles(HashMap::from([(
            (ws.as_uuid(), owner.as_uuid()),
            WorkspaceRole::Owner,
        )]))),
        vault: Arc::new(warptalk_ai_plane::DisabledVault(
            "AI_KEYS_KEK is not set".into(),
        )),
        validator: Arc::new(LlmKeyValidator::new(BaseUrlPolicy::Strict)),
        notifier: Arc::new(NoopNotifier),
    });
    let app = Router::new().nest("/api/v1", module.router());
    let tok = token(&jwt, owner);
    let (s, v, _) = call(
        &app,
        "GET",
        &format!("/api/v1/ai/workspaces/{ws}/config"),
        None,
        &tok,
    )
    .await;
    assert_eq!((s, &v["byoEnabled"]), (StatusCode::OK, &json!(false)));
    let (s, v, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/ai/workspaces/{ws}/credentials/openai"),
        Some(json!({"apiKey": "sk-whatever-1234"})),
        &tok,
    )
    .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("BYO_DISABLED"))
    );
    assert!(
        base64::engine::general_purpose::STANDARD
            .decode(KEK_B64)
            .is_ok()
    );
}
