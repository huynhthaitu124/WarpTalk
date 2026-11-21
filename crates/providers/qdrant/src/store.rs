use async_trait::async_trait;

use crate::error::VectorError;
use crate::filter::Filter;
use crate::model::{CollectionInfo, CollectionSpec, Point, ScoredPoint, SearchRequest};

/// Vector database port. A missing collection reads as empty (search, count) and is a no-op
/// for deletes; only `upsert` reports [`VectorError::CollectionNotFound`].
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Create the collection (with payload indexes) if absent. Refuses to reuse an existing
    /// collection whose dimensions differ from `spec.dims`.
    async fn ensure_collection(&self, name: &str, spec: &CollectionSpec)
    -> Result<(), VectorError>;
    async fn collection_info(&self, name: &str) -> Result<Option<CollectionInfo>, VectorError>;
    async fn upsert(&self, collection: &str, points: Vec<Point>) -> Result<(), VectorError>;
    async fn search(
        &self,
        collection: &str,
        req: &SearchRequest,
    ) -> Result<Vec<ScoredPoint>, VectorError>;
    async fn delete_by_filter(&self, collection: &str, filter: &Filter) -> Result<(), VectorError>;
    async fn count(&self, collection: &str, filter: Option<&Filter>) -> Result<u64, VectorError>;
    async fn delete_collection(&self, name: &str) -> Result<(), VectorError>;
}
