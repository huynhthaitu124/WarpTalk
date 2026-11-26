//! Ingestion: purge/blocking rules, PII gate, chunking, embedding, upsert, facts, job
//! bookkeeping with retry / dead-letter.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use warptalk_kernel::{AppError, Clock};
use warptalk_llm::{EmbeddingProvider, LlmProvider};
use warptalk_qdrant::{
    CollectionSpec, Condition, Distance, Filter, PayloadIndexType, Point, VectorError, VectorStore,
};
use warptalk_settings::{NS_RAG, ScopeChain, SettingsStore};

use super::facts::{ExtractParams, FactExtractor};
use super::ports::{
    ChunkRecord, FactRecord, JobStart, JobStatus, KnowledgeRepository, ResultPublisher, SourceKey,
};
use crate::domain::pii::{self, PiiPolicy};
use crate::domain::{
    ChunkDraft, ChunkKind, ChunkPayload, EmbeddingProfile, FactCategory, IndexContent,
    IndexRequest, IndexResult, PageText, PurgeRequest, RagConfig, RawChunk, ResultStatus,
    SegmentInput, SourceType, chunk_document, chunk_transcript, fields, normalize_id,
};

pub struct IndexingDeps {
    pub repo: Arc<dyn KnowledgeRepository>,
    pub vectors: Arc<dyn VectorStore>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    /// Provider name recorded in the embedding profile (e.g. `openai`).
    pub embedding_provider: String,
    /// True when `embeddings` sends text to a third party (honours `external_llm_allowed`).
    pub embedding_is_external: bool,
    pub settings: SettingsStore,
    pub results: Arc<dyn ResultPublisher>,
    pub clock: Arc<dyn Clock>,
    pub llm: Option<Arc<dyn LlmProvider>>,
}

pub struct IndexingService {
    d: IndexingDeps,
}

/// Failure classes of one attempt.
#[derive(Debug, Clone, thiserror::Error)]
pub enum IndexError {
    /// Retrying cannot help (bad request, dimension mismatch, ...).
    #[error("{0}")]
    Permanent(String),
    /// Provider / store / database hiccup: retry with backoff.
    #[error("{0}")]
    Transient(String),
}

impl From<VectorError> for IndexError {
    fn from(e: VectorError) -> Self {
        if e.is_retryable() {
            Self::Transient(e.to_string())
        } else {
            Self::Permanent(e.to_string())
        }
    }
}

impl From<AppError> for IndexError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Validation { .. } | AppError::Forbidden(_) | AppError::NotFound(_) => {
                Self::Permanent(e.to_string())
            }
            _ => Self::Transient(e.to_string()),
        }
    }
}

/// What the stream handler should do with the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDisposition {
    /// Processed (including blocked / deleted): ack.
    Done,
    /// Leave pending for redelivery.
    Retry(String),
    /// Attempts exhausted or permanent failure: dead-letter.
    Dead(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PurgeReport {
    pub collections: usize,
    pub vectors_deleted: u64,
    pub chunk_rows: u64,
    pub fact_rows: u64,
}

struct Outcome {
    status: ResultStatus,
    reason: String,
    chunks: usize,
    facts: usize,
    pii_masked: usize,
    pii_blocked: usize,
    profile: EmbeddingProfile,
}

impl Outcome {
    fn simple(status: ResultStatus, reason: &str, profile: EmbeddingProfile) -> Self {
        Self {
            status,
            reason: reason.to_owned(),
            chunks: 0,
            facts: 0,
            pii_masked: 0,
            pii_blocked: 0,
            profile,
        }
    }
}

/// A chunk ready for payload construction.
struct Prepared {
    id: Uuid,
    draft: ChunkDraft,
    ordinal: u32,
    title: Option<String>,
    meeting_id: Option<String>,
    revision: Option<String>,
    fact: Option<(String, FactCategory)>,
}

fn derive_id(parts: &[&str]) -> Uuid {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0x1f]);
    }
    let digest = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x80;
    b[8] = (b[8] & 0x3f) | 0x80;
    Uuid::from_bytes(b)
}

pub(crate) fn text_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn meta_str(m: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match m.get(*k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

fn meta_u64(m: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    match m.get(key)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn raw_to_prepared(req: &IndexRequest, source_id: &str, raw: &RawChunk, ordinal: u32) -> Prepared {
    let m = &raw.metadata;
    let id = Uuid::parse_str(raw.id.trim()).unwrap_or_else(|_| {
        derive_id(&[
            &req.workspace_id.to_string(),
            req.source_type.as_str(),
            source_id,
            &raw.id,
        ])
    });
    let heading_path = match m.get("heading_path") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(s)) if !s.is_empty() => {
            s.split('>').map(|p| p.trim().to_owned()).collect()
        }
        _ => Vec::new(),
    };
    let mut segment_ids: Vec<String> = match m.get("segment_ids") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    if let Some(s) = meta_str(m, &["segment_id"]) {
        segment_ids.push(s);
    }
    let speakers = meta_str(m, &["speaker_name", "speaker"])
        .into_iter()
        .collect();
    let fact = meta_str(m, &["fact"])
        .zip(meta_str(m, &["fact_category"]).and_then(|c| FactCategory::parse(&c)));
    Prepared {
        id,
        ordinal: meta_u64(m, "chunk_index").map_or(ordinal, |n| n as u32),
        title: meta_str(m, &["document_name", "title", "meeting_title"]),
        meeting_id: meta_str(m, &["meeting_id", "translation_room_id"]),
        revision: meta_str(m, &["ingestion_revision", "revision"]),
        draft: ChunkDraft {
            text: raw.text.clone(),
            page: meta_u64(m, "page").map(|p| p as u32),
            heading_path,
            start_ms: meta_u64(m, "start_ms"),
            end_ms: meta_u64(m, "end_ms"),
            segment_ids,
            speakers,
        },
        fact,
    }
}

#[derive(Default)]
struct PiiCounters {
    masked: usize,
    blocked: usize,
}

/// Apply the PII policy to a unit of text; `None` when blocked.
fn gate(text: &str, policy: PiiPolicy, c: &mut PiiCounters) -> Option<String> {
    let out = pii::apply_policy(text, policy);
    if out.blocked() {
        c.blocked += 1;
    } else if !out.hits.is_empty() {
        c.masked += out.hits.len();
    }
    out.text
}

fn gate_pages(pages: &[PageText], policy: PiiPolicy, c: &mut PiiCounters) -> Vec<PageText> {
    pages
        .iter()
        .map(|p| {
            // Paragraph granularity, so `block` drops a paragraph, not a whole page.
            let text = p
                .text
                .split("\n\n")
                .filter_map(|para| gate(para, policy, c))
                .collect::<Vec<_>>()
                .join("\n\n");
            PageText { page: p.page, text }
        })
        .collect()
}

fn gate_segments(
    segs: &[SegmentInput],
    policy: PiiPolicy,
    c: &mut PiiCounters,
) -> Vec<SegmentInput> {
    segs.iter()
        .filter_map(|s| gate(&s.text, policy, c).map(|text| SegmentInput { text, ..s.clone() }))
        .collect()
}

impl IndexingService {
    pub fn new(d: IndexingDeps) -> Self {
        Self { d }
    }

    pub(crate) async fn config(&self, workspace: Uuid) -> Result<RagConfig, IndexError> {
        Ok(self
            .d
            .settings
            .get::<RagConfig>(
                NS_RAG,
                ScopeChain::workspace(warptalk_kernel::WorkspaceId::from_uuid(workspace)),
            )
            .await?)
    }

    /// Process one `embedding:index_requests` entry with job bookkeeping.
    pub async fn handle(&self, req: &IndexRequest, message_id: &str) -> JobDisposition {
        let ws = req.workspace_id;
        let source_id = normalize_id(&req.source_id);
        let cfg = match self.config(ws).await {
            Ok(c) => c,
            Err(e) => return JobDisposition::Retry(e.to_string()),
        };
        let now = self.d.clock.now();
        let attempts = match self
            .d
            .repo
            .begin_job(
                ws,
                &req.job_id,
                req.source_type,
                &source_id,
                message_id,
                now,
            )
            .await
        {
            Ok(JobStart::Started { attempts }) => attempts,
            Ok(JobStart::Backoff { until }) => {
                return JobDisposition::Retry(format!("backing off until {until}"));
            }
            Err(e) => return JobDisposition::Retry(e.to_string()),
        };

        match self.process(req, &source_id, &cfg).await {
            Ok(o) => {
                let status = match o.status {
                    ResultStatus::Indexed => JobStatus::Indexed,
                    ResultStatus::Blocked => JobStatus::Blocked,
                    ResultStatus::Deleted => JobStatus::Deleted,
                    ResultStatus::Failed => JobStatus::Dead,
                };
                let reason = (!o.reason.is_empty()).then_some(o.reason.as_str());
                if let Err(e) = self
                    .d
                    .repo
                    .finish_job(ws, &req.job_id, status, o.chunks as u32, reason, now)
                    .await
                {
                    tracing::error!(error = %e, job = %req.job_id, "finish_job failed");
                }
                self.publish(req, &o, attempts).await;
                JobDisposition::Done
            }
            Err(IndexError::Permanent(msg)) => self.dead(req, &cfg, attempts, msg, now).await,
            Err(IndexError::Transient(msg)) => {
                if attempts >= cfg.ext.retry_max_attempts {
                    return self
                        .dead(
                            req,
                            &cfg,
                            attempts,
                            format!("attempts exhausted ({attempts}): {msg}"),
                            now,
                        )
                        .await;
                }
                let next = now + chrono::Duration::milliseconds(cfg.backoff_ms(attempts) as i64);
                if let Err(e) = self.d.repo.fail_attempt(ws, &req.job_id, &msg, next).await {
                    tracing::error!(error = %e, job = %req.job_id, "fail_attempt failed");
                }
                tracing::warn!(job = %req.job_id, attempts, error = %msg, "index attempt failed; will retry");
                JobDisposition::Retry(msg)
            }
        }
    }

    async fn dead(
        &self,
        req: &IndexRequest,
        cfg: &RagConfig,
        attempts: u32,
        msg: String,
        now: chrono::DateTime<chrono::Utc>,
    ) -> JobDisposition {
        if let Err(e) = self
            .d
            .repo
            .finish_job(
                req.workspace_id,
                &req.job_id,
                JobStatus::Dead,
                0,
                Some(&msg),
                now,
            )
            .await
        {
            tracing::error!(error = %e, job = %req.job_id, "finish_job(dead) failed");
        }
        let profile = self.profile_for(cfg);
        let o = Outcome::simple(ResultStatus::Failed, &msg, profile);
        self.publish(req, &o, attempts).await;
        JobDisposition::Dead(msg)
    }

    fn profile_for(&self, cfg: &RagConfig) -> EmbeddingProfile {
        EmbeddingProfile::new(
            self.d.embedding_provider.clone(),
            cfg.base.embedding_model.clone(),
            cfg.base.embedding_dimensions as usize,
            cfg.base.distance_metric.clone(),
        )
    }

    async fn publish(&self, req: &IndexRequest, o: &Outcome, attempts: u32) {
        let result = IndexResult {
            job_id: req.job_id.clone(),
            workspace_id: req.workspace_id,
            collection_id: req.collection_id.clone(),
            source_type: req.source_type,
            source_id: req.source_id.clone(),
            status: o.status,
            chunks_indexed: o.chunks as u32,
            provider: o.profile.provider.clone(),
            model: o.profile.model.clone(),
            dimensions: o.profile.dims as u32,
            reason: o.reason.clone(),
            timestamp_ms: self.d.clock.now().timestamp_millis(),
            pii_masked: o.pii_masked as u32,
            pii_blocked: o.pii_blocked as u32,
            facts: o.facts as u32,
            attempts,
        };
        if let Err(e) = self.d.results.publish(&result).await {
            tracing::error!(error = %e, job = %req.job_id, "publishing index result failed");
        }
    }

    // ---- purge --------------------------------------------------------------------------

    /// Delete a source's vectors (in every profile collection of the workspace) and rows.
    /// Both filters `None` purges the whole workspace.
    pub async fn purge_source(
        &self,
        workspace: Uuid,
        source_type: Option<SourceType>,
        source_id: Option<&str>,
    ) -> Result<PurgeReport, IndexError> {
        let source_id = source_id.map(normalize_id);
        let mut must = vec![Condition::eq(fields::WORKSPACE_ID, workspace.to_string())];
        if let Some(st) = source_type {
            must.push(Condition::eq(fields::SOURCE_TYPE, st.as_str()));
        }
        if let Some(id) = &source_id {
            must.push(Condition::eq(fields::SOURCE_ID, id.clone()));
        }
        let filter = Filter::must(must);
        let mut report = PurgeReport::default();
        for p in self.d.repo.profiles(workspace).await? {
            report.vectors_deleted += self.d.vectors.count(&p.collection, Some(&filter)).await?;
            self.d
                .vectors
                .delete_by_filter(&p.collection, &filter)
                .await?;
            report.collections += 1;
        }
        let (chunks, facts) = self
            .d
            .repo
            .purge_rows(workspace, source_type, source_id.as_deref())
            .await?;
        report.chunk_rows = chunks;
        report.fact_rows = facts;
        Ok(report)
    }

    /// Handle one `knowledge:purge_requests` entry.
    pub async fn handle_purge(&self, req: &PurgeRequest) -> JobDisposition {
        if req.source_id.is_none() && req.source_type.is_none() && !req.all {
            return JobDisposition::Dead(
                "purge request names no source; set `all` to purge a whole workspace".into(),
            );
        }
        match self
            .purge_source(req.workspace_id, req.source_type, req.source_id.as_deref())
            .await
        {
            Ok(r) => {
                tracing::info!(
                    workspace = %req.workspace_id,
                    vectors = r.vectors_deleted,
                    reason = req.reason.as_deref().unwrap_or(""),
                    "knowledge purge"
                );
                JobDisposition::Done
            }
            Err(IndexError::Transient(m)) => JobDisposition::Retry(m),
            Err(IndexError::Permanent(m)) => JobDisposition::Dead(m),
        }
    }

    // ---- one attempt ----------------------------------------------------------------------

    async fn process(
        &self,
        req: &IndexRequest,
        source_id: &str,
        cfg: &RagConfig,
    ) -> Result<Outcome, IndexError> {
        let ws = req.workspace_id;
        let profile = self.profile_for(cfg);

        // Removal and gating happen before any provider call (spec 14 G4).
        if req.deletion_state != "active" {
            self.purge_source(ws, Some(req.source_type), Some(source_id))
                .await?;
            return Ok(Outcome::simple(ResultStatus::Deleted, "", profile));
        }
        if !req.ai_retrieval_allowed {
            self.purge_source(ws, Some(req.source_type), Some(source_id))
                .await?;
            return Ok(Outcome::simple(
                ResultStatus::Blocked,
                "ai_retrieval_not_allowed",
                profile,
            ));
        }
        if req.retention_state != "active" {
            self.purge_source(ws, Some(req.source_type), Some(source_id))
                .await?;
            return Ok(Outcome::simple(
                ResultStatus::Blocked,
                "source_not_active",
                profile,
            ));
        }
        if !req.external_llm_allowed && self.d.embedding_is_external {
            return Ok(Outcome::simple(
                ResultStatus::Blocked,
                "external_llm_disabled_without_local_embedding_provider",
                profile,
            ));
        }

        let row = self.d.repo.register_profile(ws, &profile).await?;
        let collection = row.collection;
        let distance = Distance::parse(&profile.distance).unwrap_or(Distance::Cosine);
        let spec = payload_spec(profile.dims, distance);
        self.d.vectors.ensure_collection(&collection, &spec).await?;

        let mut counters = PiiCounters::default();
        let prepared = self.prepare(req, source_id, cfg, &mut counters);
        let now = self.d.clock.now();
        let now_ms = now.timestamp_millis();

        let payloads: Vec<(ChunkKind, ChunkPayload)> = prepared
            .iter()
            .map(|p| {
                let kind = if p.fact.is_some() {
                    ChunkKind::Fact
                } else {
                    ChunkKind::Content
                };
                (kind, self.payload(req, source_id, p, kind, now_ms))
            })
            .collect();

        let src = SourceKey {
            workspace: ws,
            source_type: req.source_type,
            source_id,
        };
        let mut indexed = 0usize;
        for kind in [ChunkKind::Content, ChunkKind::Fact] {
            let group: Vec<&ChunkPayload> = payloads
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, p)| p)
                .collect();
            if group.is_empty() && kind == ChunkKind::Fact {
                continue;
            }
            self.write_chunks(src, kind, &collection, profile.dims, cfg, &group, now)
                .await?;
            indexed += group.len();
        }

        let mut facts = 0usize;
        if cfg.ext.fact_extraction_enabled && req.external_llm_allowed {
            let content: Vec<&ChunkPayload> = payloads
                .iter()
                .filter(|(k, _)| *k == ChunkKind::Content)
                .map(|(_, p)| p)
                .collect();
            if let Some(llm) = &self.d.llm
                && !content.is_empty()
            {
                facts = self
                    .index_facts(
                        llm.clone(),
                        src,
                        &collection,
                        profile.dims,
                        cfg,
                        &content,
                        req,
                        now,
                    )
                    .await?;
            }
        }

        Ok(Outcome {
            status: ResultStatus::Indexed,
            reason: String::new(),
            chunks: indexed,
            facts,
            pii_masked: counters.masked,
            pii_blocked: counters.blocked,
            profile,
        })
    }

    fn prepare(
        &self,
        req: &IndexRequest,
        source_id: &str,
        cfg: &RagConfig,
        counters: &mut PiiCounters,
    ) -> Vec<Prepared> {
        let policy = if cfg.pii_applies_to(req.source_type) {
            cfg.ext.pii_policy
        } else {
            PiiPolicy::Off
        };
        let params = cfg.chunking();
        let ws = req.workspace_id.to_string();
        let mk = |drafts: Vec<ChunkDraft>, meeting_id: Option<String>| -> Vec<Prepared> {
            drafts
                .into_iter()
                .enumerate()
                .map(|(i, draft)| Prepared {
                    id: derive_id(&[
                        &ws,
                        req.source_type.as_str(),
                        source_id,
                        "content",
                        &i.to_string(),
                    ]),
                    draft,
                    ordinal: i as u32,
                    title: None,
                    meeting_id: meeting_id.clone(),
                    revision: None,
                    fact: None,
                })
                .collect()
        };
        match &req.content {
            Some(IndexContent::Document { pages }) => mk(
                chunk_document(&gate_pages(pages, policy, counters), &params),
                None,
            ),
            Some(IndexContent::Transcript {
                meeting_id,
                segments,
            }) => mk(
                chunk_transcript(&gate_segments(segments, policy, counters), &params),
                meeting_id.clone(),
            ),
            Some(IndexContent::Text { text }) => {
                let pages = gate_pages(
                    &[PageText {
                        page: None,
                        text: text.clone(),
                    }],
                    policy,
                    counters,
                );
                mk(chunk_document(&pages, &params), None)
            }
            None => req
                .chunks
                .iter()
                .enumerate()
                .filter_map(|(i, raw)| {
                    if raw.text.trim().is_empty() {
                        return None;
                    }
                    let text = gate(&raw.text, policy, counters)?;
                    let mut p = raw_to_prepared(
                        req,
                        source_id,
                        &RawChunk {
                            text,
                            ..raw.clone()
                        },
                        i as u32,
                    );
                    p.draft.text = p.draft.text.trim().to_owned();
                    Some(p)
                })
                .collect(),
        }
    }

    fn payload(
        &self,
        req: &IndexRequest,
        source_id: &str,
        p: &Prepared,
        kind: ChunkKind,
        now_ms: i64,
    ) -> ChunkPayload {
        let meeting_id = p
            .meeting_id
            .clone()
            .or_else(|| {
                (req.source_type == SourceType::MeetingSummary).then(|| source_id.to_owned())
            })
            .map(|m| normalize_id(&m));
        ChunkPayload {
            workspace_id: req.workspace_id,
            chunk_id: p.id,
            source_type: req.source_type,
            source_id: source_id.to_owned(),
            kind,
            ordinal: p.ordinal,
            text: p.draft.text.clone(),
            title: p.title.clone().or_else(|| req.title.clone()),
            page: p.draft.page,
            heading_path: p.draft.heading_path.clone(),
            start_ms: p.draft.start_ms,
            end_ms: p.draft.end_ms,
            segment_ids: p.draft.segment_ids.clone(),
            speakers: p.draft.speakers.clone(),
            meeting_id,
            revision: p.revision.clone().or_else(|| req.revision.clone()),
            ai_retrieval: true,
            retention_state: "active".to_owned(),
            deletion_state: "active".to_owned(),
            indexed_at_ms: now_ms,
            fact: p.fact.as_ref().map(|(f, _)| f.clone()),
            fact_category: p.fact.as_ref().map(|(_, c)| *c),
            confidence: None,
            quote: None,
            source_chunk_ids: Vec::new(),
        }
    }

    /// Embed `payloads`, upsert them, drop this source's stale points of `kind`, mirror to
    /// Postgres.
    #[allow(clippy::too_many_arguments)]
    async fn write_chunks(
        &self,
        src: SourceKey<'_>,
        kind: ChunkKind,
        collection: &str,
        dims: usize,
        cfg: &RagConfig,
        payloads: &[&ChunkPayload],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), IndexError> {
        let texts: Vec<String> = payloads.iter().map(|p| p.searchable_text()).collect();
        let vectors = self.embed_all(cfg, dims, &texts).await?;
        let points: Vec<Point> = payloads
            .iter()
            .zip(vectors)
            .map(|(p, v)| Point {
                id: p.chunk_id.to_string(),
                vector: v,
                payload: p.to_value(),
            })
            .collect();
        if !points.is_empty() {
            self.d.vectors.upsert(collection, points).await?;
        }
        // Stale points: same source and kind, not among the new ids.
        let stale = Filter {
            must: vec![
                Condition::eq(fields::WORKSPACE_ID, src.workspace.to_string()),
                Condition::eq(fields::SOURCE_TYPE, src.source_type.as_str()),
                Condition::eq(fields::SOURCE_ID, src.source_id),
                Condition::eq(fields::KIND, kind.as_str()),
            ],
            must_not: if payloads.is_empty() {
                Vec::new()
            } else {
                vec![Condition::HasId(
                    payloads.iter().map(|p| p.chunk_id.to_string()).collect(),
                )]
            },
            should: Vec::new(),
        };
        self.d.vectors.delete_by_filter(collection, &stale).await?;
        let records: Vec<ChunkRecord> = payloads
            .iter()
            .map(|p| ChunkRecord {
                id: p.chunk_id,
                ordinal: p.ordinal,
                kind,
                text_hash: text_hash(&p.text),
                payload: p.to_value(),
            })
            .collect();
        self.d
            .repo
            .replace_chunks(src, kind, collection, &records, now)
            .await?;
        Ok(())
    }

    async fn embed_all(
        &self,
        cfg: &RagConfig,
        dims: usize,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, IndexError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let batch = (cfg.base.embedding_batch_size as usize).max(1);
        let conc = (cfg.base.embedding_concurrency as usize).max(1);
        let timeout = Duration::from_millis(u64::from(cfg.base.embedding_timeout_ms));
        let model = cfg.base.embedding_model.clone();
        let batches: Vec<Vec<String>> = texts.chunks(batch).map(<[String]>::to_vec).collect();
        let results: Vec<Result<Vec<Vec<f32>>, IndexError>> = futures::stream::iter(batches)
            .map(|inputs: Vec<String>| {
                let model = model.clone();
                async move {
                    let r = tokio::time::timeout(timeout, self.d.embeddings.embed(&model, &inputs))
                        .await
                        .map_err(|_| IndexError::Transient("embedding timed out".into()))?
                        .map_err(|e| {
                            if e.is_retryable() {
                                IndexError::Transient(format!("embedding failed: {e}"))
                            } else {
                                IndexError::Permanent(format!("embedding failed: {e}"))
                            }
                        })?;
                    if r.vectors.len() != inputs.len() {
                        return Err(IndexError::Permanent(format!(
                            "embedding provider returned {} vectors for {} inputs",
                            r.vectors.len(),
                            inputs.len()
                        )));
                    }
                    Ok(r.vectors)
                }
            })
            .buffered(conc)
            .collect()
            .await;
        let mut out = Vec::with_capacity(texts.len());
        for r in results {
            out.extend(r?);
        }
        for (i, v) in out.iter().enumerate() {
            if v.len() != dims {
                return Err(IndexError::Permanent(format!(
                    "embedding dimension mismatch: expected {dims}, got {} at index {i}",
                    v.len()
                )));
            }
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    async fn index_facts(
        &self,
        llm: Arc<dyn LlmProvider>,
        src: SourceKey<'_>,
        collection: &str,
        dims: usize,
        cfg: &RagConfig,
        content: &[&ChunkPayload],
        req: &IndexRequest,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<usize, IndexError> {
        if cfg.ext.fact_model.is_empty() {
            tracing::warn!("fact extraction enabled but `fact_model` is empty; skipping");
            return Ok(0);
        }
        let model = cfg.ext.fact_model.clone();
        let chunks: Vec<(Uuid, String)> = content
            .iter()
            .map(|p| (p.chunk_id, p.text.clone()))
            .collect();
        let extractor = FactExtractor::new(llm);
        let facts = match extractor
            .extract(
                &ExtractParams {
                    model: &model,
                    title: req.title.as_deref(),
                    max_facts: cfg.base.max_facts as usize,
                    max_input_chars: cfg.base.max_fact_input_chars as usize,
                    min_confidence: cfg.ext.fact_min_confidence,
                },
                &chunks,
            )
            .await
        {
            Ok(f) => f,
            Err(e) => {
                // Facts are best effort: keep the previous ones and do not fail the job.
                tracing::warn!(error = %e, "fact extraction failed");
                return Ok(0);
            }
        };

        let by_id = |id: &Uuid| content.iter().find(|p| p.chunk_id == *id).copied();
        let ws = src.workspace.to_string();
        let mut payloads: Vec<ChunkPayload> = Vec::new();
        for (i, f) in facts.iter().enumerate() {
            let Some(origin) = f.source_chunk_ids.first().and_then(by_id) else {
                continue;
            };
            let mut p = origin.clone();
            p.chunk_id = derive_id(&[
                &ws,
                src.source_type.as_str(),
                src.source_id,
                "fact",
                &i.to_string(),
            ]);
            p.kind = ChunkKind::Fact;
            p.ordinal = i as u32;
            p.text = f.fact.clone();
            p.fact = Some(f.fact.clone());
            p.fact_category = Some(f.category);
            p.confidence = Some(f.confidence);
            p.quote = f.quote.clone();
            p.source_chunk_ids = f.source_chunk_ids.clone();
            payloads.push(p);
        }
        let refs: Vec<&ChunkPayload> = payloads.iter().collect();
        self.write_chunks(src, ChunkKind::Fact, collection, dims, cfg, &refs, now)
            .await?;
        let records: Vec<FactRecord> = facts
            .iter()
            .zip(&payloads)
            .map(|(f, p)| FactRecord {
                id: Uuid::now_v7(),
                workspace_id: src.workspace,
                source_type: src.source_type,
                source_id: src.source_id.to_owned(),
                category: f.category,
                fact: f.fact.clone(),
                quote: f.quote.clone(),
                confidence: f.confidence,
                source_chunk_ids: f.source_chunk_ids.clone(),
                chunk_id: Some(p.chunk_id),
            })
            .collect();
        self.d.repo.replace_facts(src, &records, now).await?;
        Ok(records.len())
    }
}

/// Collection layout: every filtered payload field is indexed.
pub fn payload_spec(dims: usize, distance: Distance) -> CollectionSpec {
    CollectionSpec::new(dims, distance)
        .with_index(fields::WORKSPACE_ID, PayloadIndexType::Keyword)
        .with_index(fields::SOURCE_TYPE, PayloadIndexType::Keyword)
        .with_index(fields::SOURCE_ID, PayloadIndexType::Keyword)
        .with_index(fields::MEETING_ID, PayloadIndexType::Keyword)
        .with_index(fields::KIND, PayloadIndexType::Keyword)
        .with_index(fields::PAGE, PayloadIndexType::Integer)
        .with_index(fields::AI_RETRIEVAL, PayloadIndexType::Bool)
        .with_index(fields::RETENTION_STATE, PayloadIndexType::Keyword)
        .with_index(fields::DELETION_STATE, PayloadIndexType::Keyword)
}
