//! Qdrant vector search over REST: collections with payload indexes, upserts, filtered
//! search, delete-by-filter and count behind the [`VectorStore`] trait, plus an in-memory
//! cosine implementation for tests.

mod error;
mod filter;
mod memory;
mod model;
mod rest;
mod store;

pub use error::VectorError;
pub use filter::{Condition, Filter};
pub use memory::InMemoryVectorStore;
pub use model::{
    CollectionInfo, CollectionSpec, Distance, PayloadIndexType, Point, ScoredPoint, SearchRequest,
};
pub use rest::QdrantClient;
pub use store::VectorStore;
