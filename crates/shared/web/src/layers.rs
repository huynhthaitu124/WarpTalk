//! Common middleware: request id, tracing, CORS.

use axum::Router;
use axum::http::{HeaderName, HeaderValue};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Adds request-id, trace and (when origins are given) CORS layers.
pub fn common_layers<S>(router: Router<S>, cors_origins: &[String]) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let mut router = router;
    if !cors_origins.is_empty() {
        let origins: Vec<HeaderValue> = cors_origins
            .iter()
            .filter_map(|o| match HeaderValue::from_str(o) {
                Ok(v) => Some(v),
                Err(_) => {
                    tracing::warn!(origin = %o, "ignoring invalid CORS origin");
                    None
                }
            })
            .collect();
        router = router.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods(Any)
                .allow_headers(Any)
                .allow_credentials(false),
        );
    }
    router
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test]
    async fn sets_request_id_and_cors() {
        let r = common_layers(
            Router::new().route("/", get(crate::health)),
            &["https://a.example".to_owned()],
        );
        let resp = r
            .oneshot(
                Request::get("/")
                    .header("origin", "https://a.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.headers().contains_key("x-request-id"));
        assert!(resp.headers().contains_key("access-control-allow-origin"));
    }
}
