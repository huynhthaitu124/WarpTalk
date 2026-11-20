//! Application error type shared across crates.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{message}")]
    Validation { code: String, message: String },
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{message}")]
    Conflict { code: String, message: String },
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// Explicit status and code for cases the fixed variants cannot express
    /// (e.g. 403 `EMAIL_NOT_VERIFIED`). Statuses >= 500 hide the message like `Internal`.
    #[error("{message}")]
    Coded {
        status: u16,
        code: String,
        message: String,
    },
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn validation(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Conflict {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self::RateLimited(msg.into())
    }

    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::Unavailable(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn coded(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Coded {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    /// HTTP status code.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Validation { .. } => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict { .. } => 409,
            Self::RateLimited(_) => 429,
            Self::Unavailable(_) => 503,
            Self::Internal(_) => 500,
            Self::Coded { status, .. } => *status,
        }
    }

    /// Stable machine-readable code.
    pub fn code(&self) -> &str {
        match self {
            Self::Validation { code, .. } | Self::Conflict { code, .. } => code,
            Self::Unauthorized(_) => "UNAUTHORIZED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::RateLimited(_) => "RATE_LIMITED",
            Self::Unavailable(_) => "UNAVAILABLE",
            Self::Internal(_) => "INTERNAL_ERROR",
            Self::Coded { code, .. } => code,
        }
    }

    /// Client-safe message (Internal is never leaked).
    #[cfg(any(feature = "axum", test))]
    fn public_message(&self) -> &str {
        match self {
            Self::Validation { message, .. } | Self::Conflict { message, .. } => message,
            Self::Unauthorized(m)
            | Self::Forbidden(m)
            | Self::NotFound(m)
            | Self::RateLimited(m)
            | Self::Unavailable(m) => m,
            Self::Coded { status, .. } if *status >= 500 => "internal error",
            Self::Coded { message, .. } => message,
            Self::Internal(_) => "internal error",
        }
    }

    #[cfg(any(feature = "axum", test))]
    fn body(&self) -> serde_json::Value {
        serde_json::json!({ "code": self.code(), "message": self.public_message() })
    }
}

#[cfg(feature = "axum")]
impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = axum::http::StatusCode::from_u16(self.status_code())
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, axum::Json(self.body())).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_and_codes() {
        let cases = [
            (AppError::validation("BAD_EMAIL", "x"), 400, "BAD_EMAIL"),
            (AppError::unauthorized("x"), 401, "UNAUTHORIZED"),
            (AppError::forbidden("x"), 403, "FORBIDDEN"),
            (AppError::not_found("x"), 404, "NOT_FOUND"),
            (AppError::conflict("DUP", "x"), 409, "DUP"),
            (AppError::rate_limited("x"), 429, "RATE_LIMITED"),
            (AppError::unavailable("x"), 503, "UNAVAILABLE"),
            (AppError::internal("x"), 500, "INTERNAL_ERROR"),
        ];
        for (e, status, code) in cases {
            assert_eq!(e.status_code(), status);
            assert_eq!(e.code(), code);
        }
    }

    #[test]
    fn internal_message_is_hidden() {
        let body = AppError::internal("db password leaked").body();
        assert_eq!(body["code"], "INTERNAL_ERROR");
        assert_eq!(body["message"], "internal error");
        let body = AppError::not_found("user 1").body();
        assert_eq!(body["message"], "user 1");
    }

    #[test]
    fn coded_carries_status_code_and_message() {
        let e = AppError::coded(403, "EMAIL_NOT_VERIFIED", "email address is not verified");
        assert_eq!(e.status_code(), 403);
        assert_eq!(e.code(), "EMAIL_NOT_VERIFIED");
        let body = e.body();
        assert_eq!(body["code"], "EMAIL_NOT_VERIFIED");
        assert_eq!(body["message"], "email address is not verified");
        assert_eq!(e.to_string(), "email address is not verified");
    }

    #[test]
    fn coded_server_error_hides_message() {
        let e = AppError::coded(503, "DB_DOWN", "connection to 10.0.0.5 refused");
        assert_eq!(e.status_code(), 503);
        assert_eq!(e.body()["message"], "internal error");
        let e = AppError::coded(500, "BOOM", "secret");
        assert_eq!(e.body()["code"], "BOOM");
        assert_eq!(e.body()["message"], "internal error");
    }

    #[cfg(feature = "axum")]
    #[test]
    fn coded_into_response_uses_status() {
        use axum::response::IntoResponse;
        let resp = AppError::coded(403, "EMAIL_NOT_VERIFIED", "x").into_response();
        assert_eq!(resp.status().as_u16(), 403);
        let resp = AppError::coded(42, "WEIRD", "x").into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }

    #[cfg(feature = "axum")]
    #[test]
    fn into_response_status() {
        use axum::response::IntoResponse;
        let resp = AppError::internal("secret").into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }
}
