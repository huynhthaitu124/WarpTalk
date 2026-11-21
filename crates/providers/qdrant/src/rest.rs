use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::error::VectorError;
use crate::filter::Filter;
use crate::model::{CollectionInfo, CollectionSpec, Point, ScoredPoint, SearchRequest};
use crate::store::VectorStore;

const UPSERT_BATCH: usize = 256;

/// Qdrant REST client (`/collections/...`). The API key is sent as the `api-key` header and
/// never printed.
#[derive(Clone)]
pub struct QdrantClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for QdrantClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantClient")
            .field("base", &self.base)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn transport(e: reqwest::Error) -> VectorError {
    // Strip the URL from the message: it can embed the host.
    VectorError::Transport(e.without_url().to_string())
}

impl QdrantClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Result<Self, VectorError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(transport)?;
        Ok(Self::with_client(http, base_url, api_key))
    }

    pub fn with_client(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            http,
            base: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.filter(|k| !k.is_empty()),
        }
    }

    fn check(name: &str) -> Result<(), VectorError> {
        if valid_name(name) {
            Ok(())
        } else {
            Err(VectorError::InvalidName(name.to_owned()))
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), VectorError> {
        let mut rb = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(k) = &self.api_key {
            rb = rb.header("api-key", k);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let resp = rb.send().await.map_err(transport)?;
        let status = resp.status();
        let text = resp.text().await.map_err(transport)?;
        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        if status.is_success() || status == StatusCode::NOT_FOUND || status == StatusCode::CONFLICT
        {
            return Ok((status, value));
        }
        let mut body: String = text.chars().take(500).collect();
        if body.is_empty() {
            body = status.to_string();
        }
        Err(VectorError::Status {
            status: status.as_u16(),
            body,
        })
    }

    /// Like `send` but a 404/409 is an error.
    async fn send_ok(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, VectorError> {
        let (status, v) = self.send(method, path, body).await?;
        if status.is_success() {
            Ok(v)
        } else {
            Err(VectorError::Status {
                status: status.as_u16(),
                body: v.to_string().chars().take(500).collect(),
            })
        }
    }
}

fn parse_info(v: &Value) -> Result<CollectionInfo, VectorError> {
    let r = &v["result"];
    let vectors = &r["config"]["params"]["vectors"];
    let dims = vectors["size"]
        .as_u64()
        .ok_or_else(|| VectorError::Decode("collection info has no vectors.size".into()))?;
    Ok(CollectionInfo {
        dims: dims as usize,
        points: r["points_count"].as_u64().unwrap_or(0),
    })
}

fn parse_id(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[async_trait]
impl VectorStore for QdrantClient {
    async fn ensure_collection(
        &self,
        name: &str,
        spec: &CollectionSpec,
    ) -> Result<(), VectorError> {
        if let Some(existing) = self.collection_info(name).await? {
            return if existing.dims == spec.dims {
                Ok(())
            } else {
                Err(VectorError::DimensionMismatch {
                    collection: name.to_owned(),
                    existing: existing.dims,
                    requested: spec.dims,
                })
            };
        }
        let body = json!({"vectors": {"size": spec.dims, "distance": spec.distance.as_qdrant()}});
        let (status, _) = self
            .send(Method::PUT, &format!("/collections/{name}"), Some(body))
            .await?;
        if status == StatusCode::CONFLICT {
            // Lost a creation race; the winner's dimensions must still match.
            return match self.collection_info(name).await? {
                Some(i) if i.dims != spec.dims => Err(VectorError::DimensionMismatch {
                    collection: name.to_owned(),
                    existing: i.dims,
                    requested: spec.dims,
                }),
                _ => Ok(()),
            };
        }
        if !status.is_success() {
            return Err(VectorError::Status {
                status: status.as_u16(),
                body: "collection create failed".into(),
            });
        }
        for (field, kind) in &spec.payload_indexes {
            let body = json!({"field_name": field, "field_schema": kind.as_qdrant()});
            self.send_ok(
                Method::PUT,
                &format!("/collections/{name}/index?wait=true"),
                Some(body),
            )
            .await?;
        }
        Ok(())
    }

    async fn collection_info(&self, name: &str) -> Result<Option<CollectionInfo>, VectorError> {
        Self::check(name)?;
        let (status, v) = self
            .send(Method::GET, &format!("/collections/{name}"), None)
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        parse_info(&v).map(Some)
    }

    async fn upsert(&self, collection: &str, points: Vec<Point>) -> Result<(), VectorError> {
        Self::check(collection)?;
        for batch in points.chunks(UPSERT_BATCH) {
            let pts: Vec<Value> = batch
                .iter()
                .map(|p| json!({"id": p.id, "vector": p.vector, "payload": p.payload}))
                .collect();
            let (status, v) = self
                .send(
                    Method::PUT,
                    &format!("/collections/{collection}/points?wait=true"),
                    Some(json!({"points": pts})),
                )
                .await?;
            if status == StatusCode::NOT_FOUND {
                return Err(VectorError::CollectionNotFound(collection.to_owned()));
            }
            if !status.is_success() {
                return Err(VectorError::Status {
                    status: status.as_u16(),
                    body: v.to_string().chars().take(500).collect(),
                });
            }
        }
        Ok(())
    }

    async fn search(
        &self,
        collection: &str,
        req: &SearchRequest,
    ) -> Result<Vec<ScoredPoint>, VectorError> {
        Self::check(collection)?;
        let mut body = json!({
            "vector": req.vector,
            "limit": req.limit,
            "with_payload": true,
            "with_vector": false,
        });
        if let Some(f) = req.filter.as_ref().filter(|f| !f.is_empty()) {
            body["filter"] = f.to_json();
        }
        if let Some(t) = req.score_threshold {
            body["score_threshold"] = json!(t);
        }
        let (status, v) = self
            .send(
                Method::POST,
                &format!("/collections/{collection}/points/search"),
                Some(body),
            )
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !status.is_success() {
            return Err(VectorError::Status {
                status: status.as_u16(),
                body: v.to_string().chars().take(500).collect(),
            });
        }
        let items = v["result"]
            .as_array()
            .ok_or_else(|| VectorError::Decode("search result is not an array".into()))?;
        Ok(items
            .iter()
            .map(|p| ScoredPoint {
                id: parse_id(&p["id"]),
                score: p["score"].as_f64().unwrap_or(0.0) as f32,
                payload: p.get("payload").cloned().unwrap_or(Value::Null),
            })
            .collect())
    }

    async fn delete_by_filter(&self, collection: &str, filter: &Filter) -> Result<(), VectorError> {
        Self::check(collection)?;
        if filter.is_empty() {
            // An empty filter would delete every point; never do that implicitly.
            return Ok(());
        }
        let (status, v) = self
            .send(
                Method::POST,
                &format!("/collections/{collection}/points/delete?wait=true"),
                Some(json!({"filter": filter.to_json()})),
            )
            .await?;
        if status.is_success() || status == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(VectorError::Status {
                status: status.as_u16(),
                body: v.to_string().chars().take(500).collect(),
            })
        }
    }

    async fn count(&self, collection: &str, filter: Option<&Filter>) -> Result<u64, VectorError> {
        Self::check(collection)?;
        let mut body = json!({"exact": true});
        if let Some(f) = filter.filter(|f| !f.is_empty()) {
            body["filter"] = f.to_json();
        }
        let (status, v) = self
            .send(
                Method::POST,
                &format!("/collections/{collection}/points/count"),
                Some(body),
            )
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(0);
        }
        if !status.is_success() {
            return Err(VectorError::Status {
                status: status.as_u16(),
                body: v.to_string().chars().take(500).collect(),
            });
        }
        v["result"]["count"]
            .as_u64()
            .ok_or_else(|| VectorError::Decode("count result missing".into()))
    }

    async fn delete_collection(&self, name: &str) -> Result<(), VectorError> {
        Self::check(name)?;
        let (status, v) = self
            .send(Method::DELETE, &format!("/collections/{name}"), None)
            .await?;
        if status.is_success() || status == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(VectorError::Status {
                status: status.as_u16(),
                body: v.to_string().chars().take(500).collect(),
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::filter::Condition;
    use crate::model::{Distance, PayloadIndexType};
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(s: &MockServer) -> QdrantClient {
        QdrantClient::new(s.uri(), Some("k3y".into()), Duration::from_secs(5)).unwrap()
    }

    fn info(dims: u64) -> Value {
        json!({"result": {"status": "green", "points_count": 7,
            "config": {"params": {"vectors": {"size": dims, "distance": "Cosine"}}}}})
    }

    #[tokio::test]
    async fn ensure_collection_creates_and_indexes() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/ws_1"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({"status": {"error": "nf"}})),
            )
            .mount(&s)
            .await;
        Mock::given(method("PUT"))
            .and(path("/collections/ws_1"))
            .and(header("api-key", "k3y"))
            .and(body_json(
                json!({"vectors": {"size": 4, "distance": "Cosine"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": true})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("PUT"))
            .and(path("/collections/ws_1/index"))
            .and(query_param("wait", "true"))
            .and(body_json(
                json!({"field_name": "workspace_id", "field_schema": "keyword"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": {}})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("PUT"))
            .and(path("/collections/ws_1/index"))
            .and(body_json(
                json!({"field_name": "ai_retrieval", "field_schema": "bool"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": {}})))
            .expect(1)
            .mount(&s)
            .await;
        let spec = CollectionSpec::new(4, Distance::Cosine)
            .with_index("workspace_id", PayloadIndexType::Keyword)
            .with_index("ai_retrieval", PayloadIndexType::Bool);
        client(&s).ensure_collection("ws_1", &spec).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_collection_refuses_other_dims() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/ws_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info(8)))
            .mount(&s)
            .await;
        let err = client(&s)
            .ensure_collection("ws_1", &CollectionSpec::new(4, Distance::Cosine))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            VectorError::DimensionMismatch {
                existing: 8,
                requested: 4,
                ..
            }
        ));
        assert_eq!(
            client(&s)
                .collection_info("ws_1")
                .await
                .unwrap()
                .unwrap()
                .points,
            7
        );
    }

    #[tokio::test]
    async fn upsert_search_delete_count_shapes() {
        let s = MockServer::start().await;
        let id = "0190f3c0-0000-7000-8000-000000000001";
        Mock::given(method("PUT"))
            .and(path("/collections/c/points"))
            .and(query_param("wait", "true"))
            .and(body_json(json!({"points": [
                {"id": id, "vector": [0.5, 0.5], "payload": {"text": "hi"}}
            ]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": {}})))
            .expect(1)
            .mount(&s)
            .await;
        let c = client(&s);
        c.upsert(
            "c",
            vec![Point {
                id: id.into(),
                vector: vec![0.5, 0.5],
                payload: json!({"text": "hi"}),
            }],
        )
        .await
        .unwrap();

        let filter = Filter {
            must: vec![Condition::eq("workspace_id", "w")],
            must_not: vec![Condition::eq("ai_retrieval", false)],
            should: vec![],
        };
        Mock::given(method("POST"))
            .and(path("/collections/c/points/search"))
            .and(body_json(json!({
                "vector": [1.0, 0.0], "limit": 20, "with_payload": true, "with_vector": false,
                "filter": {"must": [{"key": "workspace_id", "match": {"value": "w"}}],
                           "must_not": [{"key": "ai_retrieval", "match": {"value": false}}]}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": [
                {"id": id, "score": 0.9, "payload": {"text": "hi"}}
            ]})))
            .expect(1)
            .mount(&s)
            .await;
        let hits = c
            .search(
                "c",
                &SearchRequest::new(vec![1.0, 0.0], 20).with_filter(filter.clone()),
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);
        assert!((hits[0].score - 0.9).abs() < 1e-6);
        assert_eq!(hits[0].payload["text"], "hi");

        Mock::given(method("POST"))
            .and(path("/collections/c/points/delete"))
            .and(query_param("wait", "true"))
            .and(body_json(json!({"filter": filter.to_json()})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": {}})))
            .expect(1)
            .mount(&s)
            .await;
        c.delete_by_filter("c", &filter).await.unwrap();

        Mock::given(method("POST"))
            .and(path("/collections/c/points/count"))
            .and(body_json(
                json!({"exact": true, "filter": filter.to_json()}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"result": {"count": 12}})),
            )
            .expect(1)
            .mount(&s)
            .await;
        assert_eq!(c.count("c", Some(&filter)).await.unwrap(), 12);
    }

    #[tokio::test]
    async fn missing_collection_reads_empty_and_errors_classified() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/gone/points/search"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/collections/gone/points/count"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/collections/boom/points/search"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/collections/bad/points/search"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad filter"))
            .mount(&s)
            .await;
        let c = client(&s);
        let req = SearchRequest::new(vec![1.0], 1);
        assert!(c.search("gone", &req).await.unwrap().is_empty());
        assert_eq!(c.count("gone", None).await.unwrap(), 0);
        let e = c.search("boom", &req).await.unwrap_err();
        assert!(e.is_retryable(), "{e}");
        let e = c.search("bad", &req).await.unwrap_err();
        assert!(!e.is_retryable() && e.to_string().contains("bad filter"));
        assert!(matches!(
            c.search("a/b", &req).await,
            Err(VectorError::InvalidName(_))
        ));
        // An empty filter never becomes a delete-everything request.
        c.delete_by_filter("gone", &Filter::default())
            .await
            .unwrap();
    }

    #[test]
    fn debug_hides_api_key() {
        let c =
            QdrantClient::new("http://x", Some("secret".into()), Duration::from_secs(1)).unwrap();
        assert!(!format!("{c:?}").contains("secret"));
    }
}
