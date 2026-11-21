#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_bus::{HandlerError, StreamMessage};
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::application::{
    AccessContext, AccessScope, DocumentAccess, JobStatus, KnowledgeRepository, RelevanceKey,
    ResultPublisher, SearchFilters, run_eval,
};
use warptalk_knowledge::domain::request::{IndexRequest, ResultStatus};
use warptalk_knowledge::domain::text::tokenize;
use warptalk_knowledge::domain::{CitationKind, EvalFixture, SourceType};
use warptalk_knowledge::infrastructure::{MemoryResultPublisher, PgKnowledgeRepository};
use warptalk_knowledge::{KnowledgeDeps, KnowledgeModule, MIGRATIONS};
use warptalk_llm::{
    Capabilities, EmbeddingProvider, Embeddings, LlmError, LlmProvider, Part, ProviderId,
    Request as LlmReq, Response, StopReason, StreamEvent, Usage,
};
use warptalk_qdrant::{Condition, Filter, InMemoryVectorStore, Point, VectorStore};
use warptalk_settings::{NS_RAG, Scope, SettingsStore};
use warptalk_web::JwtKeys;

/// Hashed bag-of-words vectors; dims come from a `fake-<dims>` model name.
struct FakeEmbed {
    fail_remaining: AtomicUsize,
    calls: AtomicUsize,
}

impl FakeEmbed {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fail_remaining: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        })
    }
}

fn fake_vec(text: &str, dims: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dims];
    for t in tokenize(text) {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in t.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h % dims as u64) as usize] += 1.0;
    }
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / n).collect()
}

#[async_trait]
impl EmbeddingProvider for FakeEmbed {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let left = self.fail_remaining.load(Ordering::SeqCst);
        if left > 0 {
            self.fail_remaining.store(left - 1, Ordering::SeqCst);
            return Err(LlmError::Overloaded("fake outage".into()));
        }
        let dims: usize = model.rsplit('-').next().unwrap().parse().unwrap();
        Ok(Embeddings {
            vectors: inputs.iter().map(|t| fake_vec(t, dims)).collect(),
            dims,
            usage: Usage::default(),
        })
    }
}

struct FakeLlm(String);

#[async_trait]
impl LlmProvider for FakeLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _m: &str) -> Capabilities {
        Capabilities::conservative()
    }
    async fn complete(&self, _r: &LlmReq) -> Result<Response, LlmError> {
        Ok(Response {
            parts: vec![Part::Text(self.0.clone())],
            stop_reason: StopReason::End,
            usage: Usage::default(),
            provider_request_id: None,
        })
    }
    async fn stream(
        &self,
        _r: &LlmReq,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest("n/a".into()))
    }
    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct FakeAccess {
    docs: Vec<Uuid>,
}

#[async_trait]
impl DocumentAccess for FakeAccess {
    async fn access_scope(&self, w: WorkspaceId, u: UserId) -> AppResult<AccessScope> {
        let _ = (w, u);
        Ok(AccessScope {
            role: WorkspaceRole::Member,
            allowed_document_ids: self.docs.clone(),
            allowed_meeting_ids: vec![],
        })
    }
}

struct Env {
    module: KnowledgeModule,
    pool: sqlx::PgPool,
    settings: SettingsStore,
    vectors: Arc<InMemoryVectorStore>,
    results: Arc<MemoryResultPublisher>,
    embed: Arc<FakeEmbed>,
    repo: PgKnowledgeRepository,
    jwt: JwtKeys,
    ws: WorkspaceId,
}

async fn setup_with(
    llm: Option<&str>,
    docs: Vec<Uuid>,
    publisher: Option<Arc<dyn ResultPublisher>>,
) -> Option<Env> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let settings = SettingsStore::new(pool.clone(), None);
    let vectors = Arc::new(InMemoryVectorStore::new());
    let results = Arc::new(MemoryResultPublisher::new());
    let embed = FakeEmbed::new();
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let llm: Option<Arc<dyn LlmProvider>> = llm.map(|s| Arc::new(FakeLlm(s.to_owned())) as _);
    let module = KnowledgeModule::new(KnowledgeDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        settings: settings.clone(),
        vectors: vectors.clone(),
        embeddings: embed.clone(),
        embedding_provider: "fake".into(),
        embedding_is_external: false,
        llm,
        access: Arc::new(FakeAccess { docs }),
        results: publisher.unwrap_or_else(|| results.clone()),
    });
    let ws = WorkspaceId::new();
    let env = Env {
        module,
        repo: PgKnowledgeRepository::new(pool.clone()),
        pool,
        settings,
        vectors,
        results,
        embed,
        jwt,
        ws,
    };
    env.config(json!({})).await;
    Some(env)
}

async fn setup() -> Option<Env> {
    setup_with(None, vec![], None).await
}

impl Env {
    async fn config(&self, extra: Value) {
        let mut base = json!({
            "embedding_model": "fake-64", "embedding_dimensions": 64,
            "rerank_mode": "lexical", "mmr_lambda": 1.0, "max_chunks_per_source": 0,
            "min_score": 0.2, "retry_base_backoff_ms": 0
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        self.settings
            .put(Scope::Workspace(self.ws), NS_RAG, base, None)
            .await
            .unwrap();
    }

    fn req(&self, v: Value) -> IndexRequest {
        let mut base =
            json!({"workspace_id": self.ws.as_uuid(), "job_id": Uuid::now_v7().to_string()});
        for (k, val) in v.as_object().unwrap() {
            base[k] = val.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    async fn index(&self, v: Value) -> warptalk_knowledge::application::JobDisposition {
        let r = self.req(v);
        self.module
            .indexing()
            .handle(&r, &Uuid::now_v7().to_string())
            .await
    }

    fn admin(&self) -> AccessContext {
        AccessContext::privileged(self.ws, UserId::new(), WorkspaceRole::Owner)
    }

    async fn collection(&self) -> String {
        self.repo
            .active_profile(self.ws.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .collection
    }
}

fn doc_request(doc: Uuid) -> Value {
    json!({"source_type": "document", "source_id": doc.to_string(), "title": "Q3 Plan",
        "content": {"type": "document", "pages": [
            {"page": 1, "text": "# Budget\n\nThe quarterly budget was approved by finance."},
            {"page": 2, "text": "## Hiring\n\nWe will hire three engineers in March."}]}})
}

fn transcript_request(meeting: Uuid, segs: Value) -> Value {
    json!({"source_type": "transcript", "source_id": Uuid::now_v7().to_string(), "title": "Weekly sync",
        "content": {"type": "transcript", "meeting_id": meeting.to_string(), "segments": segs}})
}

use warptalk_knowledge::application::JobDisposition;

#[tokio::test]
async fn citations_roundtrip_upsert_search_citation() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    let meeting = Uuid::now_v7();
    assert_eq!(env.index(doc_request(doc)).await, JobDisposition::Done);
    assert_eq!(
        env.index(transcript_request(
            meeting,
            json!([
                {"segment_id": "s1", "speaker": "Ann", "start_ms": 754000, "end_ms": 760000,
                 "text": "We decided to ship the release on Friday."}
            ])
        ))
        .await,
        JobDisposition::Done
    );
    let svc = env.module.service();
    let ctx = env.admin();

    let r = svc
        .search(&ctx, "quarterly budget approved", &SearchFilters::default())
        .await
        .unwrap();
    let top = &r.chunks[0];
    assert_eq!(top.citation.kind, CitationKind::Document);
    assert_eq!(top.citation.title, "Q3 Plan");
    assert_eq!(top.citation.page, Some(1));
    assert_eq!(top.citation.label, "Q3 Plan, p. 1");
    assert_eq!(top.citation.heading_path, ["Budget"]);
    assert_eq!(
        top.citation.ref_id.as_deref(),
        Some(doc.to_string().as_str())
    );
    assert!(top.payload.text.contains("quarterly budget"));

    // page filter
    let p2 = svc
        .search(
            &ctx,
            "hire engineers",
            &SearchFilters {
                page: Some(2),
                ..SearchFilters::default()
            },
        )
        .await
        .unwrap();
    assert!(p2.chunks.iter().all(|h| h.citation.page == Some(2)));
    assert!(!p2.chunks.is_empty());

    let t = svc
        .search(
            &ctx,
            "ship the release Friday",
            &SearchFilters {
                source_types: vec![SourceType::Transcript],
                ..SearchFilters::default()
            },
        )
        .await
        .unwrap();
    let c = &t.chunks[0].citation;
    assert_eq!(c.kind, CitationKind::Transcript);
    assert_eq!(c.label, "Weekly sync @ 12:34");
    assert_eq!(c.segment_ids, ["s1"]);
    assert_eq!(c.start_ms, Some(754_000));
    assert_eq!(c.ref_id.as_deref(), Some(meeting.to_string().as_str()));

    // Postgres mirror and job table
    assert!(
        env.results
            .results()
            .iter()
            .all(|r| r.status == ResultStatus::Indexed)
    );
    let n = env
        .repo
        .count_chunks(warptalk_knowledge::application::ports::SourceKey {
            workspace: env.ws.as_uuid(),
            source_type: SourceType::Document,
            source_id: &doc.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn legacy_prechunked_requests_keep_metadata() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    let d = env
        .index(
            json!({"source_type": "document", "source_id": doc.simple().to_string(),
            "chunks": [{"id": "c1", "text": "Vendor contract renewal terms are net 30.",
                "metadata": {"document_name": "Vendors", "page": 5, "chunk_index": 0}}]}),
        )
        .await;
    assert_eq!(d, JobDisposition::Done);
    let r = env
        .module
        .service()
        .search(
            &env.admin(),
            "vendor contract renewal",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(r.chunks[0].citation.label, "Vendors, p. 5");
    // simple-form ids are normalised
    assert_eq!(r.chunks[0].payload.source_id, doc.to_string());
}

#[tokio::test]
async fn min_score_filters_and_rerank_reorders() {
    let Some(env) = setup().await else { return };
    // create profile + collection through a real index, then add crafted points
    env.index(json!({"source_type": "glossary_term", "source_id": "seed",
        "chunks": [{"id": "seed", "text": "zzz unrelated seed"}]}))
        .await;
    let coll = env.collection().await;
    let q = "quarterly budget approval";
    let qv = fake_vec(q, 64);
    // orthogonal direction
    let mut other = fake_vec("completely different words xylophone", 64);
    let dot: f32 = qv.iter().zip(&other).map(|(a, b)| a * b).sum();
    for (o, a) in other.iter_mut().zip(&qv) {
        *o -= dot * a;
    }
    let n = other.iter().map(|x| x * x).sum::<f32>().sqrt();
    other.iter_mut().for_each(|x| *x /= n);
    let mix = |c: f32| -> Vec<f32> {
        let s = (1.0 - c * c).sqrt();
        qv.iter().zip(&other).map(|(a, b)| c * a + s * b).collect()
    };
    let mk = |src: &str, text: &str, v: Vec<f32>| {
        let id = Uuid::now_v7();
        let p = warptalk_knowledge::domain::ChunkPayload {
            workspace_id: env.ws.as_uuid(),
            chunk_id: id,
            source_type: SourceType::GlossaryTerm,
            source_id: src.into(),
            kind: warptalk_knowledge::domain::ChunkKind::Content,
            ordinal: 0,
            text: text.into(),
            title: None,
            page: None,
            heading_path: vec![],
            start_ms: None,
            end_ms: None,
            segment_ids: vec![],
            speakers: vec![],
            meeting_id: None,
            revision: None,
            ai_retrieval: true,
            retention_state: "active".into(),
            deletion_state: "active".into(),
            indexed_at_ms: 0,
            fact: None,
            fact_category: None,
            confidence: None,
            quote: None,
            source_chunk_ids: vec![],
        };
        Point {
            id: id.to_string(),
            vector: v,
            payload: p.to_value(),
        }
    };
    env.vectors
        .upsert(
            &coll,
            vec![
                mk("a", "lunch menu pasta salad", mix(0.95)),
                mk("b", "the quarterly budget approval was signed", mix(0.70)),
                mk("c", "noise noise", mix(0.05)),
            ],
        )
        .await
        .unwrap();
    let svc = env.module.service();
    let ctx = env.admin();

    env.config(json!({"rerank_mode": "off"})).await;
    let off = svc
        .search(&ctx, q, &SearchFilters::default())
        .await
        .unwrap();
    let texts = |r: &warptalk_knowledge::application::SearchResult| {
        r.chunks
            .iter()
            .map(|h| h.payload.source_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(texts(&off), ["a", "b"], "min_score drops c");
    assert_eq!(off.metrics.reranker, "off");
    assert_eq!(off.metrics.candidates, 4);
    assert_eq!(off.metrics.kept_after_min_score, 2);

    env.config(json!({"rerank_mode": "lexical"})).await;
    let lex = svc
        .search(&ctx, q, &SearchFilters::default())
        .await
        .unwrap();
    assert_eq!(texts(&lex), ["b", "a"], "lexical fusion reorders");
    assert_eq!(lex.metrics.reranker, "lexical");
    assert!(lex.metrics.reranker_rank_changes >= 2);
    assert!(lex.metrics.reranker_score_delta != 0.0);
    assert!(lex.chunks[0].score > lex.chunks[1].score);
    assert!(lex.chunks[0].dense_score < lex.chunks[1].dense_score);

    // top_k comes from config
    env.config(json!({"top_k": 1})).await;
    let one = svc
        .search(&ctx, q, &SearchFilters::default())
        .await
        .unwrap();
    assert_eq!(one.chunks.len(), 1);

    // metrics registry saw all of it
    let m = svc.metrics().snapshot();
    assert_eq!(m.queries, 3);
    assert!(m.avg_latency_ms >= 0.0 && m.avg_candidates > 0.0);
}

#[tokio::test]
async fn mmr_spreads_results_across_documents() {
    let Some(env) = setup().await else { return };
    env.config(json!({"mmr_lambda": 0.5, "max_chunks_per_source": 1, "top_k": 3}))
        .await;
    for (i, name) in ["Alpha", "Beta"].iter().enumerate() {
        let doc = Uuid::now_v7();
        env.index(
            json!({"source_type": "document", "source_id": doc.to_string(), "title": name,
            "chunks": [
              {"id": format!("a{i}"), "text": format!("{name} roadmap launch plan details one")},
              {"id": format!("b{i}"), "text": format!("{name} roadmap launch plan details two")}]}),
        )
        .await;
    }
    let r = env
        .module
        .service()
        .search(
            &env.admin(),
            "roadmap launch plan",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    let sources: std::collections::HashSet<_> = r
        .chunks
        .iter()
        .map(|h| h.payload.source_id.clone())
        .collect();
    assert_eq!(r.chunks.len(), 2);
    assert_eq!(sources.len(), 2, "one chunk per document");
}

#[tokio::test]
async fn purge_when_ai_retrieval_disabled_and_scope_enforced() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    let other = Uuid::now_v7();
    env.index(doc_request(doc)).await;
    env.index(doc_request(other)).await;
    let coll = env.collection().await;
    let svc = env.module.service();
    let member = AccessContext::member(env.ws, UserId::new(), vec![doc], vec![]);

    let r = svc
        .search(&member, "quarterly budget", &SearchFilters::default())
        .await
        .unwrap();
    assert!(!r.chunks.is_empty());
    assert!(
        r.chunks
            .iter()
            .all(|h| h.payload.source_id == doc.to_string()),
        "scoped to allowlist"
    );
    // a member with no allowlist (fail closed) sees nothing
    let mut none = member.clone();
    none.allowed_document_ids = None;
    assert!(
        svc.search(&none, "quarterly budget", &SearchFilters::default())
            .await
            .unwrap()
            .chunks
            .is_empty()
    );

    // another workspace never sees it
    let foreign =
        AccessContext::privileged(WorkspaceId::new(), UserId::new(), WorkspaceRole::Owner);
    assert!(
        svc.search(&foreign, "quarterly budget", &SearchFilters::default())
            .await
            .unwrap()
            .chunks
            .is_empty()
    );

    // ai_retrieval_allowed -> false purges vectors and rows
    let before = env.vectors.count(&coll, None).await.unwrap();
    assert_eq!(before, 4);
    let d = env
        .index(
            json!({"source_type": "document", "source_id": doc.to_string(),
            "ai_retrieval_allowed": false, "chunks": []}),
        )
        .await;
    assert_eq!(d, JobDisposition::Done);
    let by_doc = Filter::must(vec![Condition::eq("source_id", doc.to_string())]);
    assert_eq!(env.vectors.count(&coll, Some(&by_doc)).await.unwrap(), 0);
    assert_eq!(env.vectors.count(&coll, None).await.unwrap(), 2);
    let last = env.results.results().pop().unwrap();
    assert_eq!(last.status, ResultStatus::Blocked);
    assert_eq!(last.reason, "ai_retrieval_not_allowed");
    let rows = env
        .repo
        .count_chunks(warptalk_knowledge::application::ports::SourceKey {
            workspace: env.ws.as_uuid(),
            source_type: SourceType::Document,
            source_id: &doc.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let admin = env.admin();
    let after = svc
        .search(&admin, "quarterly budget", &SearchFilters::default())
        .await
        .unwrap();
    assert!(
        after
            .chunks
            .iter()
            .all(|h| h.payload.source_id != doc.to_string())
    );

    // stale point left behind with ai_retrieval=false is still never returned
    let mut stale = after.chunks[0].payload.clone();
    stale.chunk_id = Uuid::now_v7();
    stale.source_id = doc.to_string();
    stale.ai_retrieval = false;
    env.vectors
        .upsert(
            &coll,
            vec![Point {
                id: stale.chunk_id.to_string(),
                vector: fake_vec("quarterly budget approved", 64),
                payload: stale.to_value(),
            }],
        )
        .await
        .unwrap();
    let again = svc
        .search(
            &admin,
            "quarterly budget approved",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    assert!(again.chunks.iter().all(|h| h.chunk_id != stale.chunk_id));

    // purge-stream handler deletes by filter, too
    let mut f = HashMap::new();
    f.insert("workspace_id".to_owned(), env.ws.to_string());
    f.insert("source_type".to_owned(), "document".to_owned());
    f.insert("source_id".to_owned(), other.to_string());
    let msg = StreamMessage {
        stream: "knowledge:purge_requests".into(),
        id: "1-0".into(),
        fields: f,
    };
    env.module.purge_handler().handle(&msg).await.unwrap();
    assert_eq!(
        env.vectors
            .count(
                &coll,
                Some(&Filter::must(vec![Condition::eq(
                    "source_id",
                    other.to_string()
                )]))
            )
            .await
            .unwrap(),
        0
    );
    // a purge naming nothing is rejected unless `all`
    let mut f = HashMap::new();
    f.insert("workspace_id".to_owned(), env.ws.to_string());
    let msg = StreamMessage {
        stream: "x".into(),
        id: "2-0".into(),
        fields: f,
    };
    assert!(matches!(
        env.module.purge_handler().handle(&msg).await,
        Err(HandlerError::Fatal(_))
    ));
}

#[tokio::test]
async fn deletion_state_deleted_removes_vectors() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    env.index(doc_request(doc)).await;
    let coll = env.collection().await;
    assert_eq!(env.vectors.count(&coll, None).await.unwrap(), 2);
    env.index(json!({"source_type": "document", "source_id": doc.to_string(), "deletion_state": "deleted"}))
        .await;
    assert_eq!(env.vectors.count(&coll, None).await.unwrap(), 0);
    assert_eq!(
        env.results.results().last().unwrap().status,
        ResultStatus::Deleted
    );
}

#[tokio::test]
async fn reindex_removes_stale_tail_chunks() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    env.index(doc_request(doc)).await;
    let coll = env.collection().await;
    assert_eq!(env.vectors.count(&coll, None).await.unwrap(), 2);
    env.index(json!({"source_type": "document", "source_id": doc.to_string(),
        "content": {"type": "document", "pages": [{"page": 1, "text": "Only one short page now."}]}}))
        .await;
    assert_eq!(env.vectors.count(&coll, None).await.unwrap(), 1);
}

#[tokio::test]
async fn pii_is_masked_before_indexing_and_block_drops_segments() {
    let Some(env) = setup().await else { return };
    let meeting = Uuid::now_v7();
    let segs = json!([
        {"segment_id": "s1", "speaker": "Ann", "start_ms": 0, "end_ms": 4000,
         "text": "Email me at jane.doe@example.com or call 0912 345 678."},
        {"segment_id": "s2", "speaker": "Bob", "start_ms": 5000, "end_ms": 9000,
         "text": "Card 4111 1111 1111 1111 please."}
    ]);
    env.index(transcript_request(meeting, segs.clone())).await;
    let coll = env.collection().await;
    let all: String = env
        .vectors
        .payloads(&coll)
        .await
        .into_iter()
        .map(|(_, p)| p.to_string())
        .collect();
    assert!(
        !all.contains("jane.doe@example.com") && !all.contains("0912") && !all.contains("4111")
    );
    assert!(all.contains("[EMAIL]") && all.contains("[PHONE]") && all.contains("[CARD]"));
    let res = env.results.results().pop().unwrap();
    assert_eq!(res.pii_masked, 3);
    // Postgres mirror is masked too
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM knowledge.chunks WHERE workspace_id = $1 AND payload::text LIKE '%jane.doe%'",
    )
    .bind(env.ws.as_uuid())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0);

    // block policy drops the offending segments entirely
    env.config(json!({"pii_policy": "block"})).await;
    let m2 = Uuid::now_v7();
    let mut with_clean = segs.as_array().unwrap().clone();
    with_clean.push(json!({"segment_id": "s3", "speaker": "Ann", "start_ms": 10000, "end_ms": 12000, "text": "Plain sentence about roadmap."}));
    env.index(transcript_request(m2, Value::Array(with_clean)))
        .await;
    let res = env.results.results().pop().unwrap();
    assert_eq!(res.pii_blocked, 2);
    assert_eq!(res.chunks_indexed, 1);
    let all: String = env
        .vectors
        .payloads(&coll)
        .await
        .into_iter()
        .map(|(_, p)| p.to_string())
        .collect();
    assert!(all.contains("Plain sentence about roadmap"));
}

#[tokio::test]
async fn retry_with_backoff_then_dead_letter() {
    let Some(env) = setup().await else { return };
    env.config(json!({"retry_max_attempts": 3, "retry_base_backoff_ms": 0}))
        .await;
    env.embed.fail_remaining.store(100, Ordering::SeqCst);
    let req = env.req(doc_request(Uuid::now_v7()));
    let mut f = HashMap::new();
    f.insert("data".to_owned(), serde_json::to_string(&req).unwrap());
    let msg = StreamMessage {
        stream: "embedding:index_requests".into(),
        id: "100-0".into(),
        fields: f,
    };
    let h = env.module.index_handler();
    for expected in 1..=2u32 {
        let r = h.handle(&msg).await;
        assert!(matches!(r, Err(HandlerError::Retry(_))), "{r:?}");
        let job = env
            .repo
            .job(env.ws.as_uuid(), &req.job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobStatus::Retrying);
        assert_eq!(job.attempts, expected);
        assert!(job.last_error.unwrap().contains("fake outage"));
    }
    assert!(env.results.results().is_empty(), "no result until final");
    let r = h.handle(&msg).await;
    assert!(matches!(r, Err(HandlerError::Fatal(_))), "{r:?}");
    let job = env
        .repo
        .job(env.ws.as_uuid(), &req.job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.attempts, 3);
    let results = env.results.results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ResultStatus::Failed);
    assert_eq!(results[0].attempts, 3);
}

#[tokio::test]
async fn backoff_defers_redelivery_and_recovery_succeeds() {
    let Some(env) = setup().await else { return };
    env.config(json!({"retry_max_attempts": 5, "retry_base_backoff_ms": 600_000, "retry_max_backoff_ms": 600_000}))
        .await;
    env.embed.fail_remaining.store(1, Ordering::SeqCst);
    let req = env.req(doc_request(Uuid::now_v7()));
    let svc = env.module.indexing();
    assert!(matches!(
        svc.handle(&req, "7-0").await,
        JobDisposition::Retry(_)
    ));
    let calls = env.embed.calls.load(Ordering::SeqCst);
    // redelivered immediately: still inside the backoff window, no provider call, no attempt
    assert!(
        matches!(svc.handle(&req, "7-0").await, JobDisposition::Retry(m) if m.contains("backing off"))
    );
    assert_eq!(env.embed.calls.load(Ordering::SeqCst), calls);
    let job = env
        .repo
        .job(env.ws.as_uuid(), &req.job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.attempts, 1);
    // permanent errors dead-letter immediately (dimension mismatch: model says 64, config 32)
    env.config(json!({"embedding_dimensions": 32})).await;
    let req2 = env.req(doc_request(Uuid::now_v7()));
    assert!(
        matches!(svc.handle(&req2, "8-0").await, JobDisposition::Dead(m) if m.contains("dimension mismatch"))
    );
}

#[tokio::test]
async fn embedding_profile_switch_uses_new_collection_and_refuses_mixing() {
    let Some(env) = setup().await else { return };
    let doc = Uuid::now_v7();
    env.index(doc_request(doc)).await;
    let old = env.collection().await;
    assert!(old.starts_with(&format!("workspace_{}_", env.ws)));
    env.config(json!({"embedding_model": "fake-32", "embedding_dimensions": 32}))
        .await;
    env.index(doc_request(doc)).await;
    let profiles = env.repo.profiles(env.ws.as_uuid()).await.unwrap();
    assert_eq!(profiles.len(), 2);
    let building = profiles
        .iter()
        .find(|p| p.status == warptalk_knowledge::application::ports::ProfileStatus::Building)
        .unwrap();
    assert_ne!(building.collection, old);
    assert_eq!(
        env.collection().await,
        old,
        "search keeps using the active profile"
    );
    let names = env.vectors.collection_names().await;
    assert!(names.contains(&old) && names.contains(&building.collection));
    // mixing dims in one collection is refused
    let err = env
        .vectors
        .ensure_collection(
            &old,
            &warptalk_knowledge::application::indexing::payload_spec(
                32,
                warptalk_qdrant::Distance::Cosine,
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        warptalk_qdrant::VectorError::DimensionMismatch { .. }
    ));
    // queries still work against the old (active) profile
    env.config(json!({"embedding_model": "fake-64", "embedding_dimensions": 64}))
        .await;
    let r = env
        .module
        .service()
        .search(&env.admin(), "quarterly budget", &SearchFilters::default())
        .await
        .unwrap();
    assert!(!r.chunks.is_empty());
    // after the reindex, activate the new profile
    env.module
        .activate_profile(env.ws, &building.profile.hash())
        .await
        .unwrap();
    assert_eq!(env.collection().await, building.collection);
    // purge removes the source from every profile collection
    let rep = env
        .module
        .indexing()
        .purge_source(
            env.ws.as_uuid(),
            Some(SourceType::Document),
            Some(&doc.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(rep.collections, 2);
    assert_eq!(rep.vectors_deleted, 4);
}

#[tokio::test]
async fn facts_are_extracted_grounded_and_replaced() {
    let answer = json!({"facts": [
        {"fact": "The release ships on Friday", "category": "decision", "quote": "ship the release on Friday", "confidence": 0.9, "chunks": [1]},
        {"fact": "Invented", "category": "bogus", "quote": "x", "confidence": 0.9, "chunks": [1]},
        {"fact": "Unsupported claim", "category": "risk", "quote": "never said", "confidence": 0.9, "chunks": []}
    ]})
    .to_string();
    let Some(env) = setup_with(Some(&answer), vec![], None).await else {
        return;
    };
    env.config(json!({"fact_extraction_enabled": true, "fact_model": "m"}))
        .await;
    let doc = Uuid::now_v7();
    let body = json!({"source_type": "document", "source_id": doc.to_string(), "title": "Notes",
        "content": {"type": "document", "pages": [{"page": 3, "text": "We decided to ship the release on Friday. Budget is fine."}]}});
    env.index(body.clone()).await;
    let facts = env
        .repo
        .list_facts(env.ws.as_uuid(), None, None, 50)
        .await
        .unwrap();
    assert_eq!(facts.len(), 1, "{facts:?}");
    let f = &facts[0];
    assert_eq!(f.category.as_str(), "decision");
    assert_eq!(f.source_chunk_ids.len(), 1);
    assert!(f.confidence > 0.8 && f.chunk_id.is_some());
    assert_eq!(env.results.results().last().unwrap().facts, 1);
    let coll = env.collection().await;
    let fact_pts = Filter::must(vec![Condition::eq("kind", "fact")]);
    assert_eq!(env.vectors.count(&coll, Some(&fact_pts)).await.unwrap(), 1);
    // re-index: facts are replaced, not duplicated
    env.index(body).await;
    assert_eq!(
        env.repo
            .list_facts(env.ws.as_uuid(), None, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(env.vectors.count(&coll, Some(&fact_pts)).await.unwrap(), 1);
    // searchable, with a knowledge citation carrying the page
    let r = env
        .module
        .service()
        .search(
            &env.admin(),
            "release ships Friday",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    let kb = r
        .chunks
        .iter()
        .find(|h| h.citation.kind == CitationKind::Knowledge)
        .unwrap();
    assert_eq!(kb.citation.page, Some(3));
    assert_eq!(
        kb.payload.fact.as_deref(),
        Some("The release ships on Friday")
    );
    // the flag off means no extraction
    env.config(json!({"fact_extraction_enabled": false})).await;
    let other = Uuid::now_v7();
    env.index(
        json!({"source_type": "document", "source_id": other.to_string(),
        "chunks": [{"id": "x", "text": "We decided to ship the release on Friday."}]}),
    )
    .await;
    assert_eq!(env.results.results().last().unwrap().facts, 0);
}

#[tokio::test]
async fn zero_results_and_empty_query_are_recorded() {
    let Some(env) = setup().await else { return };
    let svc = env.module.service();
    // no profile yet
    let r = svc
        .search(&env.admin(), "anything", &SearchFilters::default())
        .await
        .unwrap();
    assert!(r.chunks.is_empty() && r.metrics.zero_result);
    env.index(doc_request(Uuid::now_v7())).await;
    let r = svc
        .search(&env.admin(), "xylophone zebra", &SearchFilters::default())
        .await
        .unwrap();
    assert!(r.metrics.zero_result);
    let r = svc
        .search(&env.admin(), "   ", &SearchFilters::default())
        .await
        .unwrap();
    assert!(r.chunks.is_empty());
    let r = svc
        .search(&env.admin(), "quarterly budget", &SearchFilters::default())
        .await
        .unwrap();
    assert!(!r.metrics.zero_result && r.metrics.top_score.is_some());
    let m = svc.metrics().snapshot();
    assert_eq!(m.queries, 4);
    assert_eq!(m.zero_results, 3);
    assert!((m.zero_result_rate - 0.75).abs() < 1e-9);
}

#[tokio::test]
async fn offline_eval_reports_recall_and_mrr() {
    let Some(env) = setup().await else { return };
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    env.index(
        json!({"source_type": "document", "source_id": a.to_string(), "title": "A",
        "chunks": [{"id": "1", "text": "quarterly budget approval process"}]}),
    )
    .await;
    env.index(
        json!({"source_type": "document", "source_id": b.to_string(), "title": "B",
        "chunks": [{"id": "2", "text": "vendor contract renewal terms"}]}),
    )
    .await;
    let fx = EvalFixture::from_json(&format!(
        r#"{{"cases":[{{"query":"quarterly budget approval","relevant":["{a}"]}},
                     {{"query":"vendor contract renewal","relevant":["{b}"]}}]}}"#
    ))
    .unwrap();
    let rep = run_eval(
        &env.module.service(),
        &env.admin(),
        &fx,
        3,
        RelevanceKey::SourceId,
    )
    .await
    .unwrap();
    assert_eq!(rep.cases, 2);
    assert_eq!(rep.recall_at_k, 1.0);
    assert_eq!(rep.mrr, 1.0);
}

#[tokio::test]
async fn http_search_requires_auth_and_applies_allowlist() {
    let doc = Uuid::now_v7();
    let other = Uuid::now_v7();
    let Some(env) = setup_with(None, vec![doc], None).await else {
        return;
    };
    env.index(doc_request(doc)).await;
    env.index(doc_request(other)).await;
    let app = env.module.router();
    let ws = env.ws;
    let uri =
        format!("/knowledge/search?workspaceId={ws}&q=quarterly%20budget&sourceType=document");
    let res = app
        .clone()
        .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let token = env
        .jwt
        .issue_access(UserId::new(), Uuid::now_v7(), false, Utc::now())
        .unwrap()
        .0;
    let res = app
        .clone()
        .oneshot(
            Request::get(&uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    let chunks = v["chunks"].as_array().unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|c| c["sourceId"] == doc.to_string()));
    assert_eq!(chunks[0]["citation"]["label"], "Q3 Plan, p. 1");
    assert!(v["metrics"]["latencyMs"].is_number());
    let bad = format!("/knowledge/search?workspaceId={ws}&q=x&sourceType=nonsense");
    let res = app
        .oneshot(
            Request::get(&bad)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn redis_consumer_end_to_end() {
    let Ok(redis_url) = std::env::var("REDIS_URL") else {
        eprintln!("REDIS_URL not set; skipping integration test");
        return;
    };
    let mut conn = warptalk_bus::connect(&redis_url).await.unwrap();
    let results_stream = format!("test:index_results:{}", Uuid::now_v7());
    let publisher: Arc<dyn ResultPublisher> = Arc::new(
        warptalk_knowledge::infrastructure::RedisResultPublisher::new(
            conn.clone(),
            &results_stream,
        ),
    );
    let Some(env) = setup_with(None, vec![], Some(publisher)).await else {
        return;
    };
    let req_stream = format!("test:index_requests:{}", Uuid::now_v7());
    let mut cfg = warptalk_bus::ConsumerConfig::new(&req_stream, "g");
    cfg.block_ms = 200;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(warptalk_bus::run_consumer(
        conn.clone(),
        cfg,
        env.module.index_handler(),
        rx,
    ));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let req = env.req(doc_request(Uuid::now_v7()));
    warptalk_bus::xadd_json(&mut conn, &req_stream, &req, 100)
        .await
        .unwrap();
    let mut status = None;
    for _ in 0..50 {
        let r: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
            .arg(&results_stream)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        if let Some(e) = r.ids.first() {
            let s: String = redis::from_redis_value(e.map.get("status").unwrap()).unwrap();
            status = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tx.send(true).unwrap();
    let _ = task.await;
    let _: i64 = redis::cmd("DEL")
        .arg(&req_stream)
        .arg(&results_stream)
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(status.as_deref(), Some("indexed"));
}
