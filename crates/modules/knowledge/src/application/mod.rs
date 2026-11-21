//! Use cases and ports.

pub mod eval;
pub mod extract;
pub mod facts;
pub mod indexing;
pub mod metrics;
pub mod model;
pub mod ports;
pub mod query;
pub mod rerank;
pub mod search;

pub use eval::{RelevanceKey, run_eval};
pub use extract::{
    ExtractError, ExtractInput, ExtractedText, ExtractorChain, MarkdownExtractor, NoopOcr,
    OcrEngine, OcrTextExtractor, PlainTextExtractor, TextExtractor,
};
pub use facts::{ExtractParams, FactExtractor};
pub use indexing::{IndexError, IndexingDeps, IndexingService, JobDisposition, PurgeReport};
pub use metrics::{MetricsRegistry, MetricsSnapshot};
pub use model::{
    AccessContext, EffectiveScope, SearchFilters, SearchHit, SearchMetrics, SearchResult,
};
pub use ports::{
    AccessScope, DocumentAccess, JobRecord, JobStatus, KnowledgeRepository, ResultPublisher,
};
pub use rerank::{LexicalReranker, LlmReranker, Reranker};
pub use search::{KnowledgeService, SearchDeps};
