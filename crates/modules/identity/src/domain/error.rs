use warptalk_kernel::AppError;

/// Error of the auth flows. `warptalk_kernel::AppError` cannot carry custom codes for 401/403,
/// so those are modelled here; everything else delegates to `AppError`.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("{message}")]
    Coded {
        status: u16,
        code: &'static str,
        message: &'static str,
    },
    #[error(transparent)]
    App(#[from] AppError),
}

impl IdentityError {
    pub fn invalid_credentials() -> Self {
        Self::Coded {
            status: 401,
            code: "INVALID_CREDENTIALS",
            message: "invalid email or password",
        }
    }

    pub fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::Coded {
            status: 401,
            code,
            message,
        }
    }

    /// 403 for a refused account status (BR-01).
    pub fn account_refused(code: &'static str) -> Self {
        let message = match code {
            "EMAIL_NOT_VERIFIED" => "email address is not verified",
            "ACCOUNT_LOCKED" => "account is temporarily locked",
            _ => "account is disabled",
        };
        Self::Coded {
            status: 403,
            code,
            message,
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::Coded { status, .. } => *status,
            Self::App(e) => e.status_code(),
        }
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Coded { code, .. } => code,
            Self::App(e) => e.code(),
        }
    }
}
