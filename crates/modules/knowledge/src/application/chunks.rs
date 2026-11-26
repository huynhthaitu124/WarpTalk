//! Chunk management behind `/workspaces/{id}/knowledge`: list, edit a fact, delete.
//!
//! The Postgres mirror (`knowledge.chunks`) is the listing source; edits and deletes are applied
//! to the vector store first (search reads its payloads) and then to the mirror.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde::Serialize;
use uuid::Uuid;
use warptalk_kernel::{AppError, AppResult};
use warptalk_llm::EmbeddingProvider;
use warptalk_qdrant::{Condition, Filter, Point, VectorStore};

use super::indexing::text_hash;
use super::ports::{ChunkCursor, ChunkListFilter, KnowledgeRepository, StoredChunk};
use crate::domain::payload::fields;
use crate::domain::{ChunkKind, ChunkPayload, FactCategory};

pub const DEFAULT_PAGE_SIZE: u32 = 50;
pub const MAX_PAGE_SIZE: u32 = 200;
const MAX_FACT_CHARS: usize = 2000;
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ChunkServiceDeps {
    pub repo: Arc<dyn KnowledgeRepository>,
    pub vectors: Arc<dyn VectorStore>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
}

pub struct ChunkService {
    d: ChunkServiceDeps,
}

/// One chunk as the management UI sees it. Unknown values are `null`, never invented.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkView {
    pub chunk_id: Uuid,
    /// `document`, `meeting_summary`, `glossary` or `workspace_context`.
    pub source_type: String,
    pub text: String,
    pub fact: Option<String>,
    pub fact_category: Option<String>,
    pub document_id: Option<String>,
    pub document_name: Option<String>,
    pub chunk_index: u32,
    pub speaker_name: Option<String>,
    pub start_ms: Option<u64>,
    pub retention_state: String,
    pub deletion_state: String,
    pub ai_retrieval: bool,
    pub source_title: Option<String>,
    pub indexed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkPage {
    pub items: Vec<ChunkView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListChunks {
    /// Raw `sourceType` query value (validated here).
    pub source_type: Option<String>,
    /// Raw `factCategory` query value (validated here).
    pub fact_category: Option<String>,
    pub cursor: Option<String>,
    pub page_size: Option<u32>,
    /// Per-caller restrictions (`None` = owner/admin).
    pub allowed_document_ids: Option<Vec<Uuid>>,
    pub allowed_meeting_ids: Option<Vec<Uuid>>,
}

/// `Some(None)` = the key was sent as `null`; `None` = absent.
#[derive(Debug, Clone, Default)]
pub struct PatchChunk {
    pub fact: Option<Option<String>>,
    pub fact_category: Option<Option<String>>,
}

fn source_type_filter(raw: &str) -> AppResult<Vec<String>> {
    let mut out = Vec::new();
    for t in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match t {
            "document" | "meeting_summary" | "workspace_context" => out.push(t.to_owned()),
            "glossary" => {
                out.push("glossary_term".to_owned());
                out.push("global_glossary_term".to_owned());
            }
            _ => {
                return Err(AppError::validation(
                    "INVALID_SOURCE_TYPE",
                    "sourceType must be one of document, meeting_summary, glossary, workspace_context",
                ));
            }
        }
    }
    Ok(out)
}

fn parse_category(raw: &str) -> AppResult<FactCategory> {
    FactCategory::parse(raw).ok_or_else(|| {
        AppError::validation(
            "INVALID_FACT_CATEGORY",
            "factCategory must be one of decision, requirement, definition, commitment, risk, reference",
        )
    })
}

fn encode_cursor(c: ChunkCursor) -> String {
    let raw = format!("{}:{}", c.indexed_at.timestamp_micros(), c.id);
    raw.bytes().map(|b| format!("{b:02x}")).collect()
}

fn decode_cursor(s: &str) -> Option<ChunkCursor> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    let raw = String::from_utf8(bytes?).ok()?;
    let (micros, id) = raw.split_once(':')?;
    Some(ChunkCursor {
        indexed_at: Utc.timestamp_micros(micros.parse().ok()?).single()?,
        id: Uuid::parse_str(id).ok()?,
    })
}

fn view(c: &StoredChunk) -> ChunkView {
    let p = ChunkPayload::from_value(&c.payload).ok();
    let is_doc = c.source_type == "document";
    let source_type = match c.source_type.as_str() {
        "glossary_term" | "global_glossary_term" => "glossary".to_owned(),
        other => other.to_owned(),
    };
    let title = p.as_ref().and_then(|p| p.title.clone());
    let speakers = p
        .as_ref()
        .map(|p| p.speakers.join(", "))
        .filter(|s| !s.is_empty());
    ChunkView {
        chunk_id: c.id,
        source_type,
        text: p.as_ref().map(|p| p.text.clone()).unwrap_or_default(),
        fact: p.as_ref().and_then(|p| p.fact.clone()),
        fact_category: p
            .as_ref()
            .and_then(|p| p.fact_category)
            .map(|c| c.as_str().to_owned()),
        document_id: is_doc.then(|| c.source_id.clone()),
        document_name: if is_doc { title.clone() } else { None },
        chunk_index: c.ordinal,
        speaker_name: speakers,
        start_ms: p.as_ref().and_then(|p| p.start_ms),
        retention_state: p
            .as_ref()
            .map_or_else(|| "active".to_owned(), |p| p.retention_state.clone()),
        deletion_state: p
            .as_ref()
            .map_or_else(|| "active".to_owned(), |p| p.deletion_state.clone()),
        ai_retrieval: p.as_ref().is_none_or(|p| p.ai_retrieval),
        source_title: title,
        indexed_at_ms: Some(c.indexed_at.timestamp_millis()),
    }
}

impl ChunkService {
    pub fn new(d: ChunkServiceDeps) -> Self {
        Self { d }
    }

    pub async fn list(&self, workspace: Uuid, q: ListChunks) -> AppResult<ChunkPage> {
        let source_types = match q.source_type.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => Some(source_type_filter(raw)?),
        };
        let fact_category = match q.fact_category.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => Some(parse_category(raw)?),
        };
        let after = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
            None => None,
            Some(c) => Some(
                decode_cursor(c)
                    .ok_or_else(|| AppError::validation("INVALID_CURSOR", "invalid cursor"))?,
            ),
        };
        if q.page_size == Some(0) {
            return Err(AppError::validation(
                "INVALID_PAGE_SIZE",
                "pageSize must be at least 1",
            ));
        }
        let size = q
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let filter = ChunkListFilter {
            source_types,
            fact_category,
            allowed_document_ids: q.allowed_document_ids,
            allowed_meeting_ids: q.allowed_meeting_ids,
        };
        let mut rows = self
            .d
            .repo
            .list_chunks(workspace, &filter, after, size + 1)
            .await?;
        let has_more = rows.len() > size as usize;
        rows.truncate(size as usize);
        let next_cursor = if has_more {
            rows.last().map(|r| {
                encode_cursor(ChunkCursor {
                    indexed_at: r.indexed_at,
                    id: r.id,
                })
            })
        } else {
            None
        };
        Ok(ChunkPage {
            items: rows.iter().map(view).collect(),
            next_cursor,
        })
    }

    async fn require(&self, workspace: Uuid, id: Uuid) -> AppResult<StoredChunk> {
        self.d
            .repo
            .get_chunk(workspace, id)
            .await?
            .ok_or_else(|| AppError::not_found("knowledge chunk not found"))
    }

    /// Edit the fact text and/or category of a fact chunk. Re-embeds the fact so retrieval stays
    /// consistent with the edited text.
    pub async fn patch(
        &self,
        workspace: Uuid,
        id: Uuid,
        patch: PatchChunk,
    ) -> AppResult<ChunkView> {
        if patch.fact.is_none() && patch.fact_category.is_none() {
            return Err(AppError::validation(
                "NOTHING_TO_UPDATE",
                "provide fact and/or factCategory",
            ));
        }
        let new_fact = match patch.fact {
            None => None,
            Some(None) => {
                return Err(AppError::validation(
                    "INVALID_FACT",
                    "fact must not be null; delete the chunk instead",
                ));
            }
            Some(Some(f)) => {
                let f = f.trim().to_owned();
                if f.is_empty() || f.chars().count() > MAX_FACT_CHARS {
                    return Err(AppError::validation(
                        "INVALID_FACT",
                        format!("fact must be 1..={MAX_FACT_CHARS} characters"),
                    ));
                }
                Some(f)
            }
        };
        let new_category = match patch.fact_category {
            None => None,
            Some(None) => {
                return Err(AppError::validation(
                    "INVALID_FACT_CATEGORY",
                    "factCategory must not be null",
                ));
            }
            Some(Some(c)) => Some(parse_category(&c)?),
        };

        let row = self.require(workspace, id).await?;
        if row.kind != ChunkKind::Fact {
            return Err(AppError::validation(
                "CHUNK_NOT_EDITABLE",
                "only fact chunks can be edited",
            ));
        }
        let mut payload = ChunkPayload::from_value(&row.payload)
            .map_err(|e| AppError::internal(format!("stored chunk payload unreadable: {e}")))?;
        let fact = new_fact
            .or_else(|| payload.fact.clone())
            .unwrap_or_default();
        let category = new_category
            .or(payload.fact_category)
            .ok_or_else(|| AppError::internal("fact chunk without category"))?;
        payload.text = fact.clone();
        payload.fact = Some(fact.clone());
        payload.fact_category = Some(category);

        let profile = self
            .d
            .repo
            .profiles(workspace)
            .await?
            .into_iter()
            .find(|p| p.collection == row.collection)
            .ok_or_else(|| AppError::internal("embedding profile of chunk collection missing"))?;
        let emb = tokio::time::timeout(
            EMBED_TIMEOUT,
            self.d
                .embeddings
                .embed(&profile.profile.model, &[payload.searchable_text()]),
        )
        .await
        .map_err(|_| AppError::unavailable("embedding timed out"))?
        .map_err(|e| {
            tracing::warn!(error = %e, "embedding failed while editing fact");
            AppError::unavailable("embedding provider unavailable")
        })?;
        let vector = emb
            .vectors
            .into_iter()
            .next()
            .filter(|v| v.len() == profile.profile.dims)
            .ok_or_else(|| AppError::internal("embedding returned an unusable vector"))?;
        let value = payload.to_value();
        self.d
            .vectors
            .upsert(
                &row.collection,
                vec![Point {
                    id: id.to_string(),
                    vector,
                    payload: value.clone(),
                }],
            )
            .await
            .map_err(vector_err)?;
        self.d
            .repo
            .update_fact_chunk(workspace, id, category, &fact, &value, &text_hash(&fact))
            .await?;
        let updated = StoredChunk {
            payload: value,
            ..row
        };
        Ok(view(&updated))
    }

    /// Remove a chunk (and the fact it carries) from the index; the source stays untouched.
    pub async fn delete(&self, workspace: Uuid, id: Uuid) -> AppResult<()> {
        let row = self.require(workspace, id).await?;
        let filter = Filter::must(vec![
            Condition::eq(fields::WORKSPACE_ID, workspace.to_string()),
            Condition::HasId(vec![id.to_string()]),
        ]);
        self.d
            .vectors
            .delete_by_filter(&row.collection, &filter)
            .await
            .map_err(vector_err)?;
        self.d.repo.delete_chunk(workspace, id).await?;
        Ok(())
    }
}

fn vector_err(e: warptalk_qdrant::VectorError) -> AppError {
    tracing::warn!(error = %e, "vector store error");
    AppError::unavailable("vector store unavailable")
}
