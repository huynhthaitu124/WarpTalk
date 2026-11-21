#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::watch;
use uuid::Uuid;
use warptalk_assistant::suggestion::{
    DataBoundary, KnowledgeQuery, KnowledgeRetriever, LlmResolver, Purpose, ResolveError,
};
use warptalk_config::JwtConfig;
use warptalk_kernel::{RoomId, SystemClock, WorkspaceId};
use warptalk_knowledge::domain::request::IndexRequest;
use warptalk_knowledge::infrastructure::MemoryResultPublisher;
use warptalk_knowledge::{KnowledgeDeps, KnowledgeModule, MIGRATIONS};
use warptalk_llm::{EmbeddingProvider, Embeddings, LlmError, ProviderId, Usage};
use warptalk_qdrant::{InMemoryVectorStore, VectorStore};
use warptalk_settings::{NS_RAG, Scope, SettingsStore};
use warptalk_web::JwtKeys;
use warptalk_worker::ai::{AiProviders, PlatformKey, PlatformKeys};
use warptalk_worker::knowledge::{DenyDocumentAccess, ServiceRetriever};
use warptalk_worker::{build_ai_plane, build_settings, join_consumers, spawn_suggestion};

const DIMS: usize = 64;

/// Hashed bag-of-words vectors.
struct FakeEmbed;

#[async_trait]
impl EmbeddingProvider for FakeEmbed {
    async fn embed(&self, _model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        let vectors = inputs
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; DIMS];
                for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
                    if w.is_empty() {
                        continue;
                    }
                    let h = w.bytes().fold(0xcbf29ce484222325u64, |h, b| {
                        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                    });
                    v[(h % DIMS as u64) as usize] += 1.0;
                }
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                v.iter().map(|x| x / n).collect()
            })
            .collect();
        Ok(Embeddings {
            vectors,
            dims: DIMS,
            usage: Usage::default(),
        })
    }
}

fn env() -> Option<(String, String)> {
    match (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL")) {
        (Ok(d), Ok(r)) => Some((d, r)),
        _ => {
            println!("DATABASE_URL / REDIS_URL unset; skipping");
            None
        }
    }
}

fn jwt() -> JwtKeys {
    JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "t".to_owned(),
        audience: "t".to_owned(),
        access_ttl_seconds: 1,
        refresh_ttl_seconds: 1,
    })
}

#[tokio::test]
async fn indexed_transcript_chunk_is_found_by_the_suggestion_retriever() {
    let Some((db, _)) = env() else { return };
    let pool = warptalk_db::connect(&db, 4).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let settings = SettingsStore::new(pool.clone(), None);
    warptalk_settings::namespaces::register_defaults(&settings);
    let vectors: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
    let module = KnowledgeModule::new(KnowledgeDeps {
        pool: pool.clone(),
        jwt: jwt(),
        clock: Arc::new(SystemClock),
        settings: settings.clone(),
        vectors,
        embeddings: Arc::new(FakeEmbed),
        embedding_provider: "fake".into(),
        embedding_is_external: false,
        llm: None,
        access: Arc::new(DenyDocumentAccess),
        results: Arc::new(MemoryResultPublisher::new()),
    });
    let ws = WorkspaceId::new();
    settings
        .put(
            Scope::Workspace(ws),
            NS_RAG,
            json!({"embedding_model": "fake-64", "embedding_dimensions": DIMS,
                   "rerank_mode": "off", "min_score": 0.1, "retry_base_backoff_ms": 0}),
            None,
        )
        .await
        .unwrap();

    let meeting = Uuid::now_v7();
    let req: IndexRequest = serde_json::from_value(json!({
        "workspace_id": ws.as_uuid(), "job_id": Uuid::now_v7().to_string(),
        "source_type": "transcript", "source_id": Uuid::now_v7().to_string(),
        "title": "Weekly sync",
        "content": {"type": "transcript", "meeting_id": meeting.to_string(), "segments": [
            {"segment_id": "s1", "speaker": "Ann", "start_ms": 1000, "end_ms": 5000,
             "text": "We decided to ship the release on Friday."}]}
    }))
    .unwrap();
    module
        .indexing()
        .handle(&req, &Uuid::now_v7().to_string())
        .await;

    let retriever = ServiceRetriever::new(module.service());
    let hits = retriever
        .retrieve(&KnowledgeQuery {
            workspace_id: ws,
            room_id: RoomId::new(),
            query: "ship the release Friday".into(),
            top_k: 3,
        })
        .await
        .unwrap();
    assert!(!hits.is_empty(), "indexed chunk not found");
    assert!(hits[0].text.contains("ship the release"));
    // Another workspace sees nothing.
    let other = retriever
        .retrieve(&KnowledgeQuery {
            workspace_id: WorkspaceId::new(),
            room_id: RoomId::new(),
            query: "ship the release Friday".into(),
            top_k: 3,
        })
        .await
        .unwrap();
    assert!(other.is_empty());
}

#[tokio::test]
async fn suggestion_worker_starts_and_stops_cleanly() {
    let Some((db, redis_url)) = env() else { return };
    let pool = warptalk_db::connect(&db, 4).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let settings = build_settings(pool.clone(), redis, &redis_url);
    let ai = AiProviders::new(settings.clone(), PlatformKeys::default());
    let plane = build_ai_plane(
        pool,
        &redis_url,
        Arc::new(InMemoryVectorStore::new()),
        ai,
        settings,
    )
    .await
    .unwrap();
    let (tx, rx) = watch::channel(false);
    let handle = spawn_suggestion(&redis_url, plane.suggestion.clone(), &rx)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!handle.is_finished());
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(10), join_consumers(vec![handle]))
        .await
        .unwrap();
}

#[tokio::test]
async fn resolver_uses_platform_keys_and_reports_external_boundary() {
    let Some((db, _)) = env() else { return };
    let pool = warptalk_db::connect(&db, 2).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS])
        .await
        .unwrap();
    let settings = SettingsStore::new(pool, None);
    warptalk_settings::namespaces::register_defaults(&settings);
    let ws = WorkspaceId::new();

    let none = AiProviders::new(settings.clone(), PlatformKeys::default());
    assert!(none.chat().is_none());
    assert!(matches!(
        none.for_purpose(ws, Purpose::SuggestionDecide).await,
        Err(ResolveError::NotConfigured)
    ));

    let keys = PlatformKeys::default().with(
        ProviderId::OpenAi,
        PlatformKey {
            api_key: "sk-test".into(),
            base_url: Some("http://127.0.0.1:9".into()),
        },
    );
    let ai = AiProviders::new(settings, keys);
    let r = ai
        .for_purpose(ws, Purpose::SuggestionGenerate)
        .await
        .unwrap();
    assert_eq!(r.boundary, DataBoundary::External);
    assert_eq!(r.model.as_deref(), Some("gpt-4.1"));
    assert_eq!(r.provider.id(), ProviderId::OpenAi);
    assert!(ai.chat().is_some());
}
