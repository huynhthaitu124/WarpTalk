use warptalk_google::{GoogleError, GoogleVerifier};
use warptalk_kernel::AppError;

use crate::application::ports::{GoogleIdVerifier, GoogleProfile};

/// [`GoogleIdVerifier`] backed by `warptalk-google` (JWKS-verified ID tokens).
pub struct GoogleTokenVerifier(pub GoogleVerifier);

#[async_trait::async_trait]
impl GoogleIdVerifier for GoogleTokenVerifier {
    async fn verify(&self, id_token: &str) -> Result<GoogleProfile, AppError> {
        match self.0.verify(id_token).await {
            Ok(g) => Ok(GoogleProfile {
                sub: g.sub,
                email: g.email,
                name: g.name,
                picture: g.picture,
            }),
            Err(GoogleError::InvalidToken(m)) => {
                tracing::info!(reason = %m, "google id token rejected");
                Err(AppError::coded(
                    401,
                    "INVALID_GOOGLE_TOKEN",
                    "invalid google id token",
                ))
            }
            Err(e) => {
                tracing::error!(error = %e, "google verification unavailable");
                Err(AppError::coded(
                    503,
                    "GOOGLE_UNAVAILABLE",
                    "google sign-in is temporarily unavailable",
                ))
            }
        }
    }
}
