//! Pure domain: chunking, PII detection, scoring, payload schema, config. No IO.

pub mod chunker;
pub mod config;
pub mod eval;
pub mod facts;
pub mod payload;
pub mod pii;
pub mod profile;
pub mod rerank;
pub mod request;
pub mod text;

pub use chunker::{
    ChunkDraft, ChunkingParams, PageText, SegmentInput, chunk_document, chunk_transcript,
    split_pages_by_markers,
};
pub use config::{RagConfig, RagExtSettings, RerankMode};
pub use eval::{EvalCase, EvalFixture, EvalReport, evaluate};
pub use facts::{ExtractedFact, FactCategory};
pub use payload::{
    ChunkKind, ChunkPayload, Citation, CitationKind, SourceType, fields, format_timestamp,
    normalize_id,
};
pub use pii::{PiiKind, PiiMatch, PiiOutcome, PiiPolicy};
pub use profile::EmbeddingProfile;
pub use request::{
    IndexContent, IndexRequest, IndexResult, PurgeRequest, RawChunk, ResultStatus,
};
