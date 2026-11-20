//! HTTP glue shared by modules: JWT sign/verify, `AuthUser` extractor, request-id/trace layers.

pub mod auth;
pub mod jwt;
pub mod layers;

pub use auth::{AuthUser, SystemAdmin};
pub use jwt::{Claims, JwtKeys};
pub use layers::common_layers;
pub use warptalk_kernel::AppError;

/// Standard handler result: JSON body or mapped `AppError`.
pub type ApiResult<T> = Result<axum::Json<T>, AppError>;

/// Liveness probe handler.
pub async fn health() -> &'static str {
    "ok"
}
