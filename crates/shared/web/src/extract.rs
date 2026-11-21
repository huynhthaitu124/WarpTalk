//! Request extractors whose rejections are JSON `{ "code", "message" }` (via [`AppError`])
//! instead of axum's plain-text bodies. Drop-in replacements for `axum::{Json, Path, Query}`.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, OptionalFromRequest, Request};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use warptalk_kernel::AppError;

/// Code of every extractor rejection.
pub const INVALID_REQUEST: &str = "INVALID_REQUEST";

fn invalid(message: String) -> AppError {
    AppError::validation(INVALID_REQUEST, message)
}

/// JSON body; malformed or mistyped bodies answer 400 `INVALID_REQUEST` (JSON).
/// Also usable as `Option<Json<T>>` (absent content type / empty request = `None`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<S, T> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        <axum::Json<T> as FromRequest<S>>::from_request(req, state)
            .await
            .map(|axum::Json(v)| Self(v))
            .map_err(|e: JsonRejection| invalid(e.body_text()))
    }
}

impl<S, T> OptionalFromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state)
            .await
            .map(|o| o.map(|axum::Json(v)| Self(v)))
            .map_err(|e: JsonRejection| invalid(e.body_text()))
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Query string; failures answer 400 `INVALID_REQUEST` (JSON).
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(v)| Self(v))
            .map_err(|e: QueryRejection| invalid(e.body_text()))
    }
}

/// Path parameters; failures (e.g. a malformed uuid) answer 400 `INVALID_REQUEST` (JSON).
#[derive(Debug, Clone, Copy, Default)]
pub struct Path<T>(pub T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(v)| Self(v))
            .map_err(|e: PathRejection| invalid(e.body_text()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct Body1 {
        n: u32,
    }

    #[derive(Deserialize)]
    struct Q {
        page: u32,
    }

    async fn h(Path(id): Path<u32>, Query(q): Query<Q>, Json(b): Json<Body1>) -> String {
        format!("{id}-{}-{}", q.page, b.n)
    }

    async fn opt(b: Option<Json<Body1>>) -> String {
        b.map_or("none".into(), |Json(b)| b.n.to_string())
    }

    async fn call(
        app: Router,
        uri: &str,
        ct: bool,
        body: &str,
    ) -> (StatusCode, serde_json::Value, String) {
        let mut r = Request::builder().method("POST").uri(uri);
        if ct {
            r = r.header("content-type", "application/json");
        }
        let resp = app
            .oneshot(r.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap();
        let st = resp.status();
        let ctype = resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let b = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v = serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null);
        (st, v, ctype)
    }

    #[tokio::test]
    async fn rejections_are_json() {
        let app = Router::new()
            .route("/x/{id}", post(h))
            .route("/opt", post(opt));
        let (st, v, _) = call(app.clone(), "/x/7?page=2", true, r#"{"n":3}"#).await;
        assert_eq!((st, v), (StatusCode::OK, serde_json::Value::Null));
        for (uri, ct, body) in [
            ("/x/abc?page=2", true, r#"{"n":3}"#),
            ("/x/7?page=zz", true, r#"{"n":3}"#),
            ("/x/7", true, r#"{"n":3}"#),
            ("/x/7?page=2", true, r#"{"n":"s"}"#),
            ("/x/7?page=2", true, "not json"),
            ("/x/7?page=2", false, r#"{"n":3}"#),
        ] {
            let (st, v, ctype) = call(app.clone(), uri, ct, body).await;
            assert!(st.is_client_error(), "{uri} {st}");
            assert_eq!(v["code"], "INVALID_REQUEST", "{uri} {v}");
            assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
            assert!(ctype.contains("json"), "{ctype}");
        }
        let (_, _, _) = call(app.clone(), "/opt", false, "").await;
        let (st, v, _) = call(app, "/opt", true, "{bad").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["code"], "INVALID_REQUEST");
    }
}
