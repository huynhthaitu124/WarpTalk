use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::VectorError;
use crate::filter::Filter;
use crate::model::{CollectionInfo, CollectionSpec, Point, ScoredPoint, SearchRequest};
use crate::store::VectorStore;

struct Collection {
    dims: usize,
    points: BTreeMap<String, (Vec<f32>, Value)>,
}

/// Brute-force cosine store for tests and local development.
#[derive(Default)]
pub struct InMemoryVectorStore {
    collections: RwLock<HashMap<String, Collection>>,
}

impl InMemoryVectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Names of existing collections (test helper).
    pub async fn collection_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.collections.read().await.keys().cloned().collect();
        v.sort();
        v
    }

    /// Payloads of all points in a collection (test helper).
    pub async fn payloads(&self, collection: &str) -> Vec<(String, Value)> {
        self.collections
            .read()
            .await
            .get(collection)
            .map(|c| {
                c.points
                    .iter()
                    .map(|(id, (_, p))| (id.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn ensure_collection(
        &self,
        name: &str,
        spec: &CollectionSpec,
    ) -> Result<(), VectorError> {
        let mut g = self.collections.write().await;
        match g.get(name) {
            Some(c) if c.dims != spec.dims => Err(VectorError::DimensionMismatch {
                collection: name.to_owned(),
                existing: c.dims,
                requested: spec.dims,
            }),
            Some(_) => Ok(()),
            None => {
                g.insert(
                    name.to_owned(),
                    Collection {
                        dims: spec.dims,
                        points: BTreeMap::new(),
                    },
                );
                Ok(())
            }
        }
    }

    async fn collection_info(&self, name: &str) -> Result<Option<CollectionInfo>, VectorError> {
        Ok(self
            .collections
            .read()
            .await
            .get(name)
            .map(|c| CollectionInfo {
                dims: c.dims,
                points: c.points.len() as u64,
            }))
    }

    async fn upsert(&self, collection: &str, points: Vec<Point>) -> Result<(), VectorError> {
        let mut g = self.collections.write().await;
        let c = g
            .get_mut(collection)
            .ok_or_else(|| VectorError::CollectionNotFound(collection.to_owned()))?;
        for p in &points {
            if p.vector.len() != c.dims {
                return Err(VectorError::VectorSize {
                    expected: c.dims,
                    got: p.vector.len(),
                });
            }
        }
        for p in points {
            c.points.insert(p.id, (p.vector, p.payload));
        }
        Ok(())
    }

    async fn search(
        &self,
        collection: &str,
        req: &SearchRequest,
    ) -> Result<Vec<ScoredPoint>, VectorError> {
        let g = self.collections.read().await;
        let Some(c) = g.get(collection) else {
            return Ok(Vec::new());
        };
        if req.vector.len() != c.dims {
            return Err(VectorError::VectorSize {
                expected: c.dims,
                got: req.vector.len(),
            });
        }
        let mut hits: Vec<ScoredPoint> = c
            .points
            .iter()
            .filter(|(id, (_, payload))| req.filter.as_ref().is_none_or(|f| f.matches(id, payload)))
            .map(|(id, (v, payload))| ScoredPoint {
                id: id.clone(),
                score: cosine(&req.vector, v),
                payload: payload.clone(),
            })
            .filter(|h| req.score_threshold.is_none_or(|t| h.score >= t))
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        hits.truncate(req.limit);
        Ok(hits)
    }

    async fn delete_by_filter(&self, collection: &str, filter: &Filter) -> Result<(), VectorError> {
        let mut g = self.collections.write().await;
        if let Some(c) = g.get_mut(collection) {
            c.points.retain(|id, (_, p)| !filter.matches(id, p));
        }
        Ok(())
    }

    async fn count(&self, collection: &str, filter: Option<&Filter>) -> Result<u64, VectorError> {
        let g = self.collections.read().await;
        Ok(g.get(collection).map_or(0, |c| {
            c.points
                .iter()
                .filter(|(id, (_, p))| filter.is_none_or(|f| f.matches(id, p)))
                .count() as u64
        }))
    }

    async fn delete_collection(&self, name: &str) -> Result<(), VectorError> {
        self.collections.write().await.remove(name);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::filter::Condition;
    use crate::model::Distance;
    use serde_json::json;

    fn spec() -> CollectionSpec {
        CollectionSpec::new(2, Distance::Cosine)
    }

    #[tokio::test]
    async fn upsert_search_filter_delete_count() {
        let s = InMemoryVectorStore::new();
        assert!(
            s.search("c", &SearchRequest::new(vec![1.0, 0.0], 3))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            s.upsert("c", vec![]).await,
            Err(VectorError::CollectionNotFound(_))
        ));
        s.ensure_collection("c", &spec()).await.unwrap();
        s.upsert(
            "c",
            vec![
                Point {
                    id: "a".into(),
                    vector: vec![1.0, 0.0],
                    payload: json!({"w": "1"}),
                },
                Point {
                    id: "b".into(),
                    vector: vec![1.0, 1.0],
                    payload: json!({"w": "2"}),
                },
                Point {
                    id: "c".into(),
                    vector: vec![0.0, 1.0],
                    payload: json!({"w": "1"}),
                },
            ],
        )
        .await
        .unwrap();
        let all = s
            .search("c", &SearchRequest::new(vec![1.0, 0.0], 10))
            .await
            .unwrap();
        assert_eq!(
            all.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        let mut req = SearchRequest::new(vec![1.0, 0.0], 10)
            .with_filter(Filter::must(vec![Condition::eq("w", "1")]));
        req.score_threshold = Some(0.5);
        let hits = s.search("c", &req).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
        assert_eq!(s.count("c", None).await.unwrap(), 3);
        s.delete_by_filter("c", &Filter::must(vec![Condition::eq("w", "1")]))
            .await
            .unwrap();
        assert_eq!(s.count("c", None).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn refuses_mixed_dimensions() {
        let s = InMemoryVectorStore::new();
        s.ensure_collection("c", &spec()).await.unwrap();
        let err = s
            .ensure_collection("c", &CollectionSpec::new(3, Distance::Cosine))
            .await
            .unwrap_err();
        assert!(matches!(err, VectorError::DimensionMismatch { .. }));
        let err = s
            .upsert(
                "c",
                vec![Point {
                    id: "x".into(),
                    vector: vec![1.0],
                    payload: json!({}),
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, VectorError::VectorSize { .. }));
    }
}
