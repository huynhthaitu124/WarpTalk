//! pdf/docx documents (`needs_extraction` + object key) become searchable: object fetch ->
//! text extraction -> chunking (page numbers, headings) -> embedding -> vector search.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;
use warptalk_kernel::{SystemClock, UserId, WorkspaceId, WorkspaceRole};
use warptalk_knowledge::application::{
    AccessContext, AccessScope, DocumentAccess, DocumentSource, DocumentSourceError,
    JobDisposition, NoopOcr, SearchFilters,
};
use warptalk_knowledge::domain::request::{IndexRequest, ResultStatus};
use warptalk_knowledge::domain::text::tokenize;
use warptalk_knowledge::infrastructure::{ExtractLimits, MemoryResultPublisher};
use warptalk_knowledge::{KnowledgeDeps, KnowledgeModule, MIGRATIONS};
use warptalk_llm::{EmbeddingProvider, Embeddings, LlmError, Usage};
use warptalk_qdrant::InMemoryVectorStore;
use warptalk_settings::{NS_RAG, Scope, SettingsStore};
use warptalk_web::JwtKeys;

const TWO_PAGES: &[u8] = include_bytes!("fixtures/two_pages.pdf");
const SCANNED: &[u8] = include_bytes!("fixtures/scanned.pdf");
const DIMS: usize = 64;

struct FakeEmbed;

#[async_trait]
impl EmbeddingProvider for FakeEmbed {
    async fn embed(&self, _model: &str, inputs: &[String]) -> Result<Embeddings, LlmError> {
        let vectors = inputs
            .iter()
            .map(|text| {
                let mut v = vec![0.0f32; DIMS];
                for t in tokenize(text) {
                    let mut h: u64 = 0xcbf29ce484222325;
                    for b in t.bytes() {
                        h ^= u64::from(b);
                        h = h.wrapping_mul(0x100000001b3);
                    }
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

struct Deny;

#[async_trait]
impl DocumentAccess for Deny {
    async fn access_scope(
        &self,
        _: WorkspaceId,
        _: UserId,
    ) -> warptalk_kernel::AppResult<AccessScope> {
        Err(warptalk_kernel::AppError::forbidden("n/a"))
    }
}

struct MapSource(HashMap<String, Vec<u8>>);

#[async_trait]
impl DocumentSource for MapSource {
    async fn fetch(&self, key: &str) -> Result<Vec<u8>, DocumentSourceError> {
        self.0
            .get(key)
            .cloned()
            .ok_or_else(|| DocumentSourceError::NotFound(key.to_owned()))
    }
}

fn docx() -> Vec<u8> {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Chính sách nghỉ phép</w:t></w:r></w:p>
<w:p><w:r><w:t>Nhân viên được nghỉ phép mười hai ngày mỗi năm.</w:t></w:r></w:p>
</w:body></w:document>"#;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("word/document.xml", o).unwrap();
        z.write_all(xml.as_bytes()).unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

struct Env {
    module: KnowledgeModule,
    results: Arc<MemoryResultPublisher>,
    ws: WorkspaceId,
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
    let settings = SettingsStore::new(pool.clone(), None);
    let results = Arc::new(MemoryResultPublisher::new());
    let jwt = JwtKeys::from_config(&warptalk_config::JwtConfig {
        secret: "0123456789abcdef0123456789abcdef".to_owned(),
        issuer: "warptalk-test".to_owned(),
        audience: "warptalk".to_owned(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 2_592_000,
    });
    let mut objects = HashMap::new();
    objects.insert("k/policy.pdf".to_owned(), TWO_PAGES.to_vec());
    objects.insert("k/scan.pdf".to_owned(), SCANNED.to_vec());
    objects.insert("k/leave.docx".to_owned(), docx());
    objects.insert("k/broken.pdf".to_owned(), b"%PDF-1.4 truncated".to_vec());
    let module = KnowledgeModule::new(KnowledgeDeps {
        pool,
        jwt,
        clock: Arc::new(SystemClock),
        settings: settings.clone(),
        vectors: Arc::new(InMemoryVectorStore::new()),
        embeddings: Arc::new(FakeEmbed),
        embedding_provider: "fake".into(),
        embedding_is_external: false,
        llm: None,
        access: Arc::new(Deny),
        results: results.clone(),
    })
    .with_document_source(
        Arc::new(MapSource(objects)),
        ExtractLimits::default(),
        Arc::new(NoopOcr),
    );
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
    Some(Env {
        module,
        results,
        ws,
    })
}

impl Env {
    async fn index(&self, key: &str, mime: &str, name: &str) -> (Uuid, JobDisposition) {
        let doc = Uuid::now_v7();
        let v: Value = json!({
            "workspace_id": self.ws.as_uuid(), "job_id": Uuid::now_v7().to_string(),
            "source_type": "document", "source_id": doc.to_string(), "title": name,
            "needs_extraction": true, "object_key": key, "mime_type": mime, "file_name": name});
        let req: IndexRequest = serde_json::from_value(v).unwrap();
        let d = self
            .module
            .indexing()
            .handle(&req, &Uuid::now_v7().to_string())
            .await;
        (doc, d)
    }

    fn last_result(&self, doc: Uuid) -> warptalk_knowledge::domain::IndexResult {
        self.results
            .results()
            .into_iter()
            .rev()
            .find(|r| r.source_id == doc.to_string())
            .unwrap()
    }

    fn ctx(&self) -> AccessContext {
        AccessContext::privileged(self.ws, UserId::new(), WorkspaceRole::Owner)
    }
}

#[tokio::test]
async fn pdf_becomes_searchable_with_page_citations() {
    let Some(env) = setup().await else { return };
    let (doc, d) = env
        .index("k/policy.pdf", "application/pdf", "Policy.pdf")
        .await;
    assert_eq!(d, JobDisposition::Done);
    let r = env.last_result(doc);
    assert_eq!(r.status, ResultStatus::Indexed);
    assert!(r.chunks_indexed >= 2, "{r:?}");

    let svc = env.module.service();
    let hits = svc
        .search(
            &env.ctx(),
            "refund within 30 days",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    let top = &hits.chunks[0];
    assert_eq!(top.citation.page, Some(1));
    assert_eq!(top.citation.label, "Policy.pdf, p. 1");
    assert!(top.payload.text.contains("refund"));

    let hits = svc
        .search(
            &env.ctx(),
            "international orders take longer",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(hits.chunks[0].citation.page, Some(2));
    assert!(hits.chunks[0].payload.text.contains("International"));
}

#[tokio::test]
async fn docx_is_searchable_with_heading_path_even_with_wrong_mime() {
    let Some(env) = setup().await else { return };
    let (doc, d) = env
        .index("k/leave.docx", "application/octet-stream", "Leave.bin")
        .await;
    assert_eq!(d, JobDisposition::Done);
    assert_eq!(env.last_result(doc).status, ResultStatus::Indexed);
    let hits = env
        .module
        .service()
        .search(
            &env.ctx(),
            "nghỉ phép mười hai ngày",
            &SearchFilters::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        hits.chunks[0].citation.heading_path,
        ["Chính sách nghỉ phép"]
    );
}

#[tokio::test]
async fn scanned_and_broken_and_missing_documents_do_not_index() {
    let Some(env) = setup().await else { return };
    // Scanned: no OCR engine -> 0 chunks (workspace marks the document `skipped`).
    let (doc, d) = env.index("k/scan.pdf", "application/pdf", "Scan.pdf").await;
    assert_eq!(d, JobDisposition::Done);
    assert_eq!(env.last_result(doc).chunks_indexed, 0);
    // Corrupt PDF and missing object: failed results, no panic.
    for key in ["k/broken.pdf", "k/missing.pdf"] {
        let (doc, d) = env.index(key, "application/pdf", "X.pdf").await;
        assert!(matches!(d, JobDisposition::Dead(_)), "{key}: {d:?}");
        assert_eq!(env.last_result(doc).status, ResultStatus::Failed);
    }
}
