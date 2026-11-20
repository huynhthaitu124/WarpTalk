//! Axum extractors for authenticated callers.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::{AUTHORIZATION, UPGRADE};
use axum::http::request::Parts;
use warptalk_kernel::{AppError, UserId};

use crate::jwt::{Claims, JwtKeys};

/// Authenticated user (valid access token).
pub struct AuthUser(pub Claims);

impl AuthUser {
    pub fn user_id(&self) -> UserId {
        self.0.sub
    }
}

fn is_websocket_upgrade(parts: &Parts) -> bool {
    parts
        .headers
        .get(UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

fn query_token(query: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "access_token").then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl<S> FromRequestParts<S> for AuthUser
where
    JwtKeys: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let keys = JwtKeys::from_ref(state);
        let missing = || AppError::Unauthorized("missing bearer token".to_owned());
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let (scheme, tok) = v.split_once(' ')?;
                scheme
                    .eq_ignore_ascii_case("bearer")
                    .then(|| tok.trim().to_owned())
            });
        let token = match header {
            Some(t) => t,
            None if is_websocket_upgrade(parts) => parts
                .uri
                .query()
                .and_then(query_token)
                .ok_or_else(missing)?,
            None => return Err(missing()),
        };
        keys.verify(&token).map(AuthUser)
    }
}

/// Authenticated system administrator (`sa == true`).
pub struct SystemAdmin(pub Claims);

impl<S> FromRequestParts<S> for SystemAdmin
where
    JwtKeys: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthUser(claims) = AuthUser::from_request_parts(parts, state).await?;
        if claims.sa {
            Ok(Self(claims))
        } else {
            Err(AppError::Forbidden("system admin required".to_owned()))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::jwt::tests::cfg;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use chrono::Utc;
    use tower::ServiceExt;
    use uuid::Uuid;

    const S: &str = "0123456789abcdef0123456789abcdef";

    fn app() -> (Router, JwtKeys) {
        let keys = JwtKeys::from_config(&cfg(S, "a"));
        let r = Router::new()
            .route(
                "/me",
                get(|u: AuthUser| async move { u.user_id().to_string() }),
            )
            .route("/admin", get(|_: SystemAdmin| async { "ok" }))
            .with_state(keys.clone());
        (r, keys)
    }

    fn token(k: &JwtKeys, sa: bool) -> String {
        k.issue_access(UserId::new(), Uuid::now_v7(), sa, Utc::now())
            .unwrap()
            .0
    }

    async fn status(r: Router, req: Request<Body>) -> StatusCode {
        r.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn bearer_flow() {
        let (r, k) = app();
        let req = Request::get("/me").body(Body::empty()).unwrap();
        assert_eq!(status(r.clone(), req).await, StatusCode::UNAUTHORIZED);
        let req = Request::get("/me")
            .header("authorization", format!("Bearer {}", token(&k, false)))
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(r.clone(), req).await, StatusCode::OK);
        let req = Request::get("/admin")
            .header("authorization", format!("Bearer {}", token(&k, false)))
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(r.clone(), req).await, StatusCode::FORBIDDEN);
        let req = Request::get("/admin")
            .header("authorization", format!("Bearer {}", token(&k, true)))
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(r, req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn query_token_only_for_websocket() {
        let (r, k) = app();
        let uri = format!("/me?x=1&access_token={}", token(&k, false));
        let req = Request::get(&uri).body(Body::empty()).unwrap();
        assert_eq!(status(r.clone(), req).await, StatusCode::UNAUTHORIZED);
        let req = Request::get(&uri)
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(r, req).await, StatusCode::OK);
    }
}
