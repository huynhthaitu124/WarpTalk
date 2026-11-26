#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_config::JwtConfig;
use warptalk_kernel::{AppError, AppResult, SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::application::ports::{ChunkRecord, FactRecord, SourceKey};
use warptalk_knowledge::application::{
    AccessScope, DocumentAccess, KnowledgeRepository, ResultPublisher,
};
use warptalk_knowledge::domain::{
    ChunkKind, ChunkPayload, EmbeddingProfile, FactCategory, SourceType,
};
use warptalk_knowledge::infrastructure::{MemoryResultPublisher, PgKnowledgeRepository};
use warptalk_knowledge::{KnowledgeDeps, KnowledgeModule, MIGRATIONS};
use warptalk_llm::{EmbeddingProvider, Embeddings, LlmError, Usage};
use warptalk_qdrant::{
    CollectionSpec, Condition, Distance, Filter, InMemoryVectorStore, Point, VectorStore,
};
use warptalk_settings::SettingsStore;
use warptalk_web::JwtKeys;

const DIMS: usize = 8;

struct FakeEmbed;

#[async_trait]
impl EmbeddingProvider for FakeEmbed {
    async fn embed(&self, _model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        Ok(Embeddings {
            vectors: inputs
                .iter()
                .map(|t| {
                    let mut v = vec![0.0f32; DIMS];
                    v[t.len() % DIMS] = 1.0;
                    v
                })
                .collect(),
            dims: DIMS,
            usage: Usage::default(),
        })
    }
}

struct Access {
    ws: Uuid,
    admin: UserId,
    member: UserId,
    docs: Vec<Uuid>,
}

#[async_trait]
impl DocumentAccess for Access {
    async fn access_scope(&self, w: WorkspaceId, u: UserId) -> AppResult<AccessScope> {
        let role = if w.as_uuid() != self.ws {
            None
        } else if u == self.admin {
            Some(WorkspaceRole::Admin)
        } else if u == self.member {
            Some(WorkspaceRole::Member)
        } else {
            None
        };
        match role {
            Some(role) => Ok(AccessScope {
                role,
                allowed_document_ids: self.docs.clone(),
                allowed_meeting_ids: vec![],
            }),
            None => Err(AppError::forbidden("not a member")),
        }
    }
}

struct Env {
    app: Router,
    repo: PgKnowledgeRepository,
    vectors: Arc<InMemoryVectorStore>,
    jwt: JwtKeys,
    ws: Uuid,
    admin: UserId,
    member: UserId,
    collection: String,
    allowed_doc: Uuid,
    other_doc: Uuid,
    base: DateTime<Utc>,
}

async fn setup() -> Option<Env> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return None;
    };
    let pool = warptalk_db::connect(&url, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[warptalk_settings::MIGRATIONS, MIGRATIONS])
        .await
        .unwrap();
    let jwt = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let ws = Uuid::now_v7();
    let (admin, member) = (UserId::new(), UserId::new());
    let (allowed_doc, other_doc) = (Uuid::now_v7(), Uuid::now_v7());
    let vectors = Arc::new(InMemoryVectorStore::new());
    let publisher: Arc<dyn ResultPublisher> = Arc::new(MemoryResultPublisher::new());
    let module = KnowledgeModule::new(KnowledgeDeps {
        pool: pool.clone(),
        jwt: jwt.clone(),
        clock: Arc::new(SystemClock),
        settings: SettingsStore::new(pool.clone(), None),
        vectors: vectors.clone(),
        embeddings: Arc::new(FakeEmbed),
        embedding_provider: "fake".into(),
        embedding_is_external: false,
        llm: None,
        access: Arc::new(Access {
            ws,
            admin,
            member,
            docs: vec![allowed_doc],
        }),
        results: publisher,
    });
    let repo = PgKnowledgeRepository::new(pool);
    let profile = repo
        .register_profile(ws, &EmbeddingProfile::new("fake", "fake-8", DIMS, "cosine"))
        .await
        .unwrap();
    vectors
        .ensure_collection(
            &profile.collection,
            &CollectionSpec::new(DIMS, Distance::Cosine),
        )
        .await
        .unwrap();
    Some(Env {
        app: module.router(),
        repo,
        vectors,
        jwt,
        ws,
        admin,
        member,
        collection: profile.collection,
        allowed_doc,
        other_doc,
        base: Utc::now() - Duration::hours(1),
    })
}

struct Seed<'a> {
    ws: Uuid,
    source_type: SourceType,
    source_id: String,
    kind: ChunkKind,
    title: &'a str,
    text: &'a str,
    fact: Option<(&'a str, FactCategory)>,
    at: DateTime<Utc>,
}

impl Env {
    fn token(&self, u: UserId) -> String {
        self.jwt
            .issue_access(u, Uuid::now_v7(), false, Utc::now())
            .unwrap()
            .0
    }

    /// Mirror row + vector point (+ fact row) as the indexer would write them.
    async fn seed(&self, s: Seed<'_>) -> Uuid {
        let id = Uuid::now_v7();
        let payload = ChunkPayload {
            workspace_id: s.ws,
            chunk_id: id,
            source_type: s.source_type,
            source_id: s.source_id.clone(),
            kind: s.kind,
            ordinal: 3,
            text: s.text.to_owned(),
            title: Some(s.title.to_owned()),
            page: None,
            heading_path: vec![],
            start_ms: (s.source_type == SourceType::MeetingSummary).then_some(90_000),
            end_ms: None,
            segment_ids: vec![],
            speakers: if s.source_type == SourceType::MeetingSummary {
                vec!["Ann".into()]
            } else {
                vec![]
            },
            meeting_id: None,
            revision: None,
            ai_retrieval: true,
            retention_state: "active".into(),
            deletion_state: "active".into(),
            indexed_at_ms: s.at.timestamp_millis(),
            fact: s.fact.map(|(f, _)| f.to_owned()),
            fact_category: s.fact.map(|(_, c)| c),
            confidence: None,
            quote: None,
            source_chunk_ids: vec![],
        };
        let src = SourceKey {
            workspace: s.ws,
            source_type: s.source_type,
            source_id: &s.source_id,
        };
        // `replace_chunks` deletes siblings of the same kind, so seed each source under its own id.
        self.repo
            .replace_chunks(
                src,
                s.kind,
                &self.collection,
                &[ChunkRecord {
                    id,
                    ordinal: 3,
                    kind: s.kind,
                    text_hash: "h".into(),
                    payload: payload.to_value(),
                }],
                s.at,
            )
            .await
            .unwrap();
        if let Some((f, c)) = s.fact {
            self.repo
                .replace_facts(
                    src,
                    &[FactRecord {
                        id: Uuid::now_v7(),
                        workspace_id: s.ws,
                        source_type: s.source_type,
                        source_id: s.source_id.clone(),
                        category: c,
                        fact: f.to_owned(),
                        quote: None,
                        confidence: 0.9,
                        source_chunk_ids: vec![],
                        chunk_id: Some(id),
                    }],
                    s.at,
                )
                .await
                .unwrap();
        }
        let mut v = vec![0.0f32; DIMS];
        v[0] = 1.0;
        self.vectors
            .upsert(
                &self.collection,
                vec![Point {
                    id: id.to_string(),
                    vector: v,
                    payload: payload.to_value(),
                }],
            )
            .await
            .unwrap();
        id
    }

    async fn doc_chunk(&self, doc: Uuid, text: &str, mins: i64) -> Uuid {
        self.seed(Seed {
            ws: self.ws,
            source_type: SourceType::Document,
            source_id: doc.to_string(),
            kind: ChunkKind::Content,
            title: "Q3 Plan",
            text,
            fact: None,
            at: self.base + Duration::minutes(mins),
        })
        .await
    }

    async fn fact_chunk(&self, fact: &str, cat: FactCategory, mins: i64) -> Uuid {
        self.seed(Seed {
            ws: self.ws,
            source_type: SourceType::MeetingSummary,
            source_id: Uuid::now_v7().to_string(),
            kind: ChunkKind::Fact,
            title: "Weekly sync",
            text: fact,
            fact: Some((fact, cat)),
            at: self.base + Duration::minutes(mins),
        })
        .await
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        user: Option<UserId>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(u) = user {
            b = b.header("authorization", format!("Bearer {}", self.token(u)));
        }
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    fn url(&self) -> String {
        format!("/workspaces/{}/knowledge", self.ws)
    }
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["chunkId"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn list_filters_scopes_and_paginates() {
    let Some(env) = setup().await else { return };
    let a = env.doc_chunk(env.allowed_doc, "alpha", 1).await;
    let b = env.doc_chunk(env.other_doc, "beta", 2).await;
    let f = env
        .fact_chunk("Ship Friday", FactCategory::Decision, 3)
        .await;
    let r = env
        .fact_chunk("Budget at risk", FactCategory::Risk, 4)
        .await;
    // Raw transcript chunk and another workspace's chunk must never show up.
    env.seed(Seed {
        ws: env.ws,
        source_type: SourceType::Transcript,
        source_id: Uuid::now_v7().to_string(),
        kind: ChunkKind::Content,
        title: "T",
        text: "spoken words",
        fact: None,
        at: env.base + Duration::minutes(5),
    })
    .await;
    env.seed(Seed {
        ws: Uuid::now_v7(),
        source_type: SourceType::Document,
        source_id: Uuid::now_v7().to_string(),
        kind: ChunkKind::Content,
        title: "Foreign",
        text: "foreign",
        fact: None,
        at: env.base + Duration::minutes(6),
    })
    .await;

    // unauthenticated / non-member
    let (st, _) = env.call("GET", &env.url(), None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = env.call("GET", &env.url(), Some(UserId::new()), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // admin: everything but the transcript, newest first
    let (st, v) = env.call("GET", &env.url(), Some(env.admin), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        ids(&v),
        [r, f, b, a].map(|i| i.to_string()),
        "newest first, no transcript, no foreign workspace"
    );
    assert!(v["nextCursor"].is_null());
    let first = &v["items"][0];
    assert_eq!(first["sourceType"], "meeting_summary");
    assert_eq!(first["fact"], "Budget at risk");
    assert_eq!(first["factCategory"], "risk");
    assert_eq!(first["speakerName"], "Ann");
    assert_eq!(first["startMs"], 90_000);
    assert_eq!(first["aiRetrieval"], true);
    assert!(first["documentId"].is_null());
    assert!(first["indexedAtMs"].is_i64());
    let doc = &v["items"][3];
    assert_eq!(doc["sourceType"], "document");
    assert_eq!(doc["documentId"], env.allowed_doc.to_string());
    assert_eq!(doc["documentName"], "Q3 Plan");
    assert_eq!(doc["sourceTitle"], "Q3 Plan");
    assert_eq!(doc["chunkIndex"], 3);
    assert!(doc["fact"].is_null() && doc["speakerName"].is_null() && doc["startMs"].is_null());

    // filters
    let (_, v) = env
        .call(
            "GET",
            &format!("{}?sourceType=document", env.url()),
            Some(env.admin),
            None,
        )
        .await;
    assert_eq!(ids(&v), [b, a].map(|i| i.to_string()));
    let (_, v) = env
        .call(
            "GET",
            &format!("{}?factCategory=decision", env.url()),
            Some(env.admin),
            None,
        )
        .await;
    assert_eq!(ids(&v), [f.to_string()]);
    let (_, v) = env
        .call(
            "GET",
            &format!("{}?sourceType=workspace_context", env.url()),
            Some(env.admin),
            None,
        )
        .await;
    assert!(ids(&v).is_empty());
    for bad in [
        "sourceType=transcript",
        "sourceType=nope",
        "factCategory=nope",
        "pageSize=0",
        "cursor=zz",
    ] {
        let (st, body) = env
            .call(
                "GET",
                &format!("{}?{bad}", env.url()),
                Some(env.admin),
                None,
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        assert!(body["code"].is_string());
    }

    // pagination: 2 per page walks all four rows without duplicates
    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let uri = format!("{}?pageSize=2&cursor={cursor}", env.url());
        let (st, v) = env.call("GET", &uri, Some(env.admin), None).await;
        assert_eq!(st, StatusCode::OK);
        seen.extend(ids(&v));
        match v["nextCursor"].as_str() {
            Some(c) => cursor = c.to_owned(),
            None => break,
        }
    }
    assert_eq!(seen, [r, f, b, a].map(|i| i.to_string()));

    // member: only the allowlisted document plus non-document sources
    let (st, v) = env.call("GET", &env.url(), Some(env.member), None).await;
    assert_eq!(st, StatusCode::OK);
    let got = ids(&v);
    assert!(got.contains(&a.to_string()));
    assert!(!got.contains(&b.to_string()));
}

#[tokio::test]
async fn patch_edits_fact_everywhere_and_requires_admin() {
    let Some(env) = setup().await else { return };
    let f = env
        .fact_chunk("Ship Friday", FactCategory::Decision, 1)
        .await;
    let c = env.doc_chunk(env.allowed_doc, "plain text", 2).await;
    let uri = format!("{}/{f}", env.url());

    let body = json!({"fact": "  Ship on Monday  ", "factCategory": "commitment"});
    let (st, _) = env
        .call("PATCH", &uri, Some(env.member), Some(body.clone()))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = env.call("PATCH", &uri, None, Some(body.clone())).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let (st, v) = env.call("PATCH", &uri, Some(env.admin), Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["chunkId"], f.to_string());
    assert_eq!(v["fact"], "Ship on Monday");
    assert_eq!(v["factCategory"], "commitment");
    assert_eq!(v["text"], "Ship on Monday");

    // mirror, fact row and vector payload agree
    let facts = env
        .repo
        .list_facts(env.ws, None, Some(FactCategory::Commitment), 10)
        .await
        .unwrap();
    assert!(
        facts
            .iter()
            .any(|x| x.chunk_id == Some(f) && x.fact == "Ship on Monday")
    );
    let payloads = env.vectors.payloads(&env.collection).await;
    let p = payloads
        .iter()
        .find(|(id, _)| *id == f.to_string())
        .map(|(_, p)| p)
        .unwrap();
    assert_eq!(p["fact"], "Ship on Monday");
    assert_eq!(p["fact_category"], "commitment");
    let (_, v) = env
        .call(
            "GET",
            &format!("{}?factCategory=commitment", env.url()),
            Some(env.admin),
            None,
        )
        .await;
    assert_eq!(ids(&v), [f.to_string()]);

    // category only keeps the fact
    let (st, v) = env
        .call(
            "PATCH",
            &uri,
            Some(env.admin),
            Some(json!({"factCategory": "risk"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["fact"], "Ship on Monday");
    assert_eq!(v["factCategory"], "risk");

    // validation
    for bad in [
        json!({}),
        json!({"fact": null}),
        json!({"fact": "   "}),
        json!({"factCategory": null}),
        json!({"factCategory": "opinion"}),
    ] {
        let (st, v) = env
            .call("PATCH", &uri, Some(env.admin), Some(bad.clone()))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        assert!(v["code"].is_string());
    }
    // content chunks carry no fact
    let (st, _) = env
        .call(
            "PATCH",
            &format!("{}/{c}", env.url()),
            Some(env.admin),
            Some(json!({"fact": "x"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = env
        .call(
            "PATCH",
            &format!("{}/{}", env.url(), Uuid::now_v7()),
            Some(env.admin),
            Some(json!({"fact": "x"})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_removes_chunk_fact_and_vector_only() {
    let Some(env) = setup().await else { return };
    let f = env
        .fact_chunk("Ship Friday", FactCategory::Decision, 1)
        .await;
    let keep = env.doc_chunk(env.allowed_doc, "keep me", 2).await;
    let uri = format!("{}/{f}", env.url());

    let (st, _) = env.call("DELETE", &uri, Some(env.member), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = env.call("DELETE", &uri, Some(UserId::new()), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _) = env.call("DELETE", &uri, Some(env.admin), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let in_index = |id: Uuid| Filter::must(vec![Condition::HasId(vec![id.to_string()])]);
    assert_eq!(
        env.vectors
            .count(&env.collection, Some(&in_index(f)))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        env.vectors
            .count(&env.collection, Some(&in_index(keep)))
            .await
            .unwrap(),
        1
    );
    assert!(env.repo.get_chunk(env.ws, f).await.unwrap().is_none());
    let facts = env.repo.list_facts(env.ws, None, None, 100).await.unwrap();
    assert!(facts.iter().all(|x| x.chunk_id != Some(f)));
    let (_, v) = env.call("GET", &env.url(), Some(env.admin), None).await;
    assert_eq!(ids(&v), [keep.to_string()]);

    let (st, _) = env.call("DELETE", &uri, Some(env.admin), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
