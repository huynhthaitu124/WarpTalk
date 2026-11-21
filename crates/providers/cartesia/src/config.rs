use std::fmt;
use std::time::Duration;

use serde::Deserialize;

use crate::error::CartesiaError;

/// Configuration of the Cartesia client. All fields have defaults so it can be stored as JSON in
/// the `pipeline` settings namespace; the API key comes from `warptalk-config`, never from code.
/// Pool size and URLs are read at construction; the timeouts are re-read per call when the
/// client is rebuilt by the owner (the client itself is cheap to swap).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CartesiaConfig {
    pub api_key: String,
    /// WebSocket endpoint without query. UNVERIFIED against the current docs: taken from the
    /// upstream SDK's behaviour (`tts/websocket`).
    pub ws_url: String,
    pub http_base_url: String,
    /// `Cartesia-Version` header. UNVERIFIED: value copied from the vendor's public examples.
    pub api_version: String,
    /// Spec 13 section 3: `sonic-3.5`.
    pub model_id: String,
    /// Output PCM rate: 44100 (spec 13 section 3) or 24000 (lighter on the egress).
    pub sample_rate: u32,
    /// Pre-dialled sockets (spec 08 section 4 item 7: 3; dial p50 427.6 ms vs 0.1 ms to open a
    /// context).
    pub pool_size: usize,
    pub connect_timeout_ms: u64,
    /// A pooled socket idle longer than this is assumed dead (upstream: 90 s, a deliberately
    /// short guess in the safe direction).
    pub max_idle_s: u64,
    /// Per-sentence bound (upstream `SENTENCE_TIMEOUT_SECONDS` 6.0): a hang is worse than an
    /// error, this is what turns one into the other.
    pub sentence_timeout_ms: u64,
    /// How long ending a context may wait for the vendor's `done`.
    pub close_ack_ms: u64,
    pub http_timeout_ms: u64,
    /// Voice catalog cache (spec 13 section 4: 21600 s).
    pub catalog_ttl_s: u64,
    pub catalog_max_scanned: usize,
}

impl Default for CartesiaConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            ws_url: "wss://api.cartesia.ai/tts/websocket".into(),
            http_base_url: "https://api.cartesia.ai".into(),
            api_version: "2025-04-16".into(),
            model_id: "sonic-3.5".into(),
            sample_rate: 44_100,
            pool_size: 3,
            connect_timeout_ms: 5_000,
            max_idle_s: 90,
            sentence_timeout_ms: 6_000,
            close_ack_ms: 1_000,
            http_timeout_ms: 15_000,
            catalog_ttl_s: 21_600,
            catalog_max_scanned: 2_000,
        }
    }
}

impl fmt::Debug for CartesiaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CartesiaConfig")
            .field("api_key", &"<redacted>")
            .field("ws_url", &self.ws_url)
            .field("model_id", &self.model_id)
            .field("sample_rate", &self.sample_rate)
            .field("pool_size", &self.pool_size)
            .finish_non_exhaustive()
    }
}

impl CartesiaConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), CartesiaError> {
        const RATES: [u32; 5] = [16_000, 22_050, 24_000, 44_100, 48_000];
        if !RATES.contains(&self.sample_rate) {
            return Err(CartesiaError::NotConfigured("sample_rate not supported"));
        }
        if self.sentence_timeout_ms == 0 || self.connect_timeout_ms == 0 {
            return Err(CartesiaError::NotConfigured("timeouts must be > 0"));
        }
        if self.model_id.is_empty() {
            return Err(CartesiaError::NotConfigured("model_id is empty"));
        }
        Ok(())
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }
    pub fn sentence_timeout(&self) -> Duration {
        Duration::from_millis(self.sentence_timeout_ms)
    }
    pub fn close_ack(&self) -> Duration {
        Duration::from_millis(self.close_ack_ms)
    }
    pub fn max_idle(&self) -> Duration {
        Duration::from_secs(self.max_idle_s)
    }
    pub fn catalog_ttl(&self) -> Duration {
        Duration::from_secs(self.catalog_ttl_s)
    }
}
