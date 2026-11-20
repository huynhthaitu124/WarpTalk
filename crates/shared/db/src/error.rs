use warptalk_kernel::AppError;

/// Map a sqlx error to an [`AppError`]. Unexpected errors are logged and hidden as Internal.
pub fn map_sqlx_error(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::RowNotFound => return AppError::not_found("resource not found"),
        sqlx::Error::PoolTimedOut => {
            return AppError::unavailable("database temporarily unavailable");
        }
        sqlx::Error::Database(db) => match db.code().as_deref() {
            Some("23505") => return AppError::conflict("DUPLICATE", "resource already exists"),
            Some("23503") => {
                return AppError::validation("INVALID_REFERENCE", "referenced resource is invalid");
            }
            _ => {}
        },
        _ => {}
    }
    tracing::error!(error = %e, "database error");
    AppError::internal(e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn maps_basic_variants() {
        assert_eq!(map_sqlx_error(sqlx::Error::RowNotFound).code(), "NOT_FOUND");
        assert_eq!(map_sqlx_error(sqlx::Error::PoolTimedOut).status_code(), 503);
        assert_eq!(map_sqlx_error(sqlx::Error::PoolClosed).status_code(), 500);
    }
}
