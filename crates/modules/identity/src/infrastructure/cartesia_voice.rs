use warptalk_cartesia::{CartesiaConfig, CartesiaError, CartesiaHttp, base_language};
use warptalk_kernel::{AppError, AppResult};

use crate::application::voice::{VoiceCloner, VoiceProviderAdmin};

/// Cartesia adapter for cloning and provider-side deletion (REST only, no sockets).
pub struct CartesiaVoice {
    http: CartesiaHttp,
}

impl CartesiaVoice {
    pub fn new(api_key: impl Into<String>) -> Result<Self, CartesiaError> {
        let cfg = CartesiaConfig::new(api_key);
        Ok(Self {
            http: CartesiaHttp::new(&cfg)?,
        })
    }
}

fn map(e: CartesiaError) -> AppError {
    AppError::unavailable(format!("voice provider error: {e}"))
}

#[async_trait::async_trait]
impl VoiceCloner for CartesiaVoice {
    fn provider(&self) -> &'static str {
        "cartesia"
    }

    async fn clone_voice(&self, wav: &[u8], name: &str, language: &str) -> AppResult<String> {
        self.http
            .clone_voice(wav, name, &base_language(language))
            .await
            .map_err(map)
    }
}

#[async_trait::async_trait]
impl VoiceProviderAdmin for CartesiaVoice {
    async fn delete_voice(&self, provider_voice_id: &str) -> AppResult<()> {
        self.http.delete_voice(provider_voice_id).await.map_err(map)
    }
}
