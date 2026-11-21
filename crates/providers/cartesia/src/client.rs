use std::sync::Arc;
use std::time::Duration;

use crate::config::CartesiaConfig;
use crate::context::{ContextParams, SpeakContext};
use crate::error::CartesiaError;
use crate::http::{CartesiaHttp, VoiceCatalog, VoiceInfo};
use crate::pool::{PoolStats, WarmPool};

/// Facade: warm socket pool + contexts + REST. Cheap to share (`Arc<CartesiaClient>`).
pub struct CartesiaClient {
    cfg: Arc<CartesiaConfig>,
    pool: Arc<WarmPool>,
    http: Arc<CartesiaHttp>,
    catalog: VoiceCatalog,
}

impl CartesiaClient {
    pub fn new(cfg: CartesiaConfig) -> Result<Self, CartesiaError> {
        cfg.validate()?;
        let cfg = Arc::new(cfg);
        let http = Arc::new(CartesiaHttp::new(&cfg)?);
        Ok(Self {
            pool: WarmPool::new(cfg.clone()),
            catalog: VoiceCatalog::new(http.clone(), cfg.catalog_ttl(), cfg.catalog_max_scanned),
            http,
            cfg,
        })
    }

    pub fn config(&self) -> &CartesiaConfig {
        &self.cfg
    }

    pub fn sample_rate(&self) -> u32 {
        self.cfg.sample_rate
    }

    /// Dial the pool; call once at startup so the first sentence finds warm sockets. Also
    /// starts the periodic sweep.
    pub async fn warm_up(&self) -> usize {
        let n = self.pool.warm_up().await;
        self.pool.spawn_maintenance(Duration::from_secs(30));
        n
    }

    pub fn pool(&self) -> &Arc<WarmPool> {
        &self.pool
    }
    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// Opens a context on a warm socket (a cold dial only when the pool is empty). Opening is
    /// local bookkeeping: the vendor learns of the context at its first push.
    pub async fn open_context(
        &self,
        voice_id: &str,
        language: &str,
    ) -> Result<SpeakContext, CartesiaError> {
        let socket = self.pool.claim().await?;
        Ok(SpeakContext::new(
            socket,
            ContextParams {
                model_id: self.cfg.model_id.clone(),
                voice_id: voice_id.to_string(),
                language: language.to_string(),
                sample_rate: self.cfg.sample_rate,
                sentence_timeout: self.cfg.sentence_timeout(),
                close_ack: self.cfg.close_ack(),
            },
        ))
    }

    pub async fn voices(&self, language: &str) -> Vec<VoiceInfo> {
        self.catalog.voices(language).await
    }

    pub async fn clone_voice(
        &self,
        wav: &[u8],
        name: &str,
        language: &str,
    ) -> Result<String, CartesiaError> {
        self.http.clone_voice(wav, name, language).await
    }

    pub async fn delete_voice(&self, voice_id: &str) -> Result<(), CartesiaError> {
        self.http.delete_voice(voice_id).await
    }
}

/// Built-in voices for the two languages upstream confirmed (spec 13 section 4 item 7).
/// Anything else must come from the catalog; never invent ids.
pub fn default_voice_id(language: &str) -> &'static str {
    match crate::http::base_language(language).as_str() {
        "vi" => "5619d38c-cf51-4d8e-9575-48f61a280413",
        _ => "694f9389-aac1-45b6-b726-9d9369183238",
    }
}
