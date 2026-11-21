//! All translation knobs. Stored as JSON in the `pipeline` settings namespace; every field has a
//! default (spec 13 section 2.1 / spec 08 section 6) so a partial document is valid. Hot reload:
//! the stage and [`crate::OpenAiTranslator`] take a `watch::Receiver<MtConfig>` and read it per
//! segment / per call; only the pool size and the socket URL are fixed at construction.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

use crate::error::MtError;

#[derive(Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct RealtimeConfig {
    /// Use the Realtime text session for the first sentence of each segment.
    pub enabled: bool,
    pub api_key: String,
    /// WebSocket base URL; `?model=` is appended when absent. UNVERIFIED shape (docs host not
    /// reachable from the sandbox); taken from the upstream integration.
    pub base_url: String,
    pub model: String,
    /// `minimal` measured best: higher effort slowed or broke it (spec 08 section 4 item 6).
    pub reasoning_effort: String,
    /// Hot sockets kept ready (spec 13: 4).
    pub pool_size: usize,
    /// Per-call bound before falling to the chat model. 2.0 s: a premature fallback turned a
    /// rare 1.2 s answer into a 5-19 s cliff (spec 08 section 4 item 6).
    pub timeout_ms: u64,
    /// 64 tokens ended `incomplete` upstream; 128 is the measured floor.
    pub max_output_tokens: u32,
    pub connect_timeout_ms: u64,
    /// OpenAI's session cap is 60 min; never hand out a socket older than this.
    pub max_socket_age_s: u64,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: String::new(),
            base_url: "wss://api.openai.com/v1/realtime".into(),
            model: "gpt-realtime-2.1-mini".into(),
            reasoning_effort: "minimal".into(),
            pool_size: 4,
            timeout_ms: 2_000,
            max_output_tokens: 128,
            connect_timeout_ms: 5_000,
            max_socket_age_s: 50 * 60,
        }
    }
}

impl fmt::Debug for RealtimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeConfig")
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("pool_size", &self.pool_size)
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

impl RealtimeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// Fallback / batch model (spec 13: `gpt-5.4-nano`).
    pub model: String,
    pub max_tokens: u32,
    /// 0.0 so a repeated sentence translates identically and hits the TTS cache.
    pub temperature: f32,
    /// Retries of a transient failure (upstream: 2, backoff 0.5 s doubling).
    pub retries: u32,
    pub backoff_ms: u64,
    pub request_timeout_ms: u64,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            model: "gpt-5.4-nano".into(),
            max_tokens: 512,
            temperature: 0.0,
            retries: 2,
            backoff_ms: 500,
            request_timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct SpeculativeConfig {
    pub enabled: bool,
    /// Unclaimed entries expire (upstream 15 s).
    pub ttl_ms: u64,
    /// Per-call bound (upstream 1.5 s). If one translation costs more, speculation is decoration.
    pub timeout_ms: u64,
    /// Concurrent speculative calls (upstream: one slot, so speculation never occupies the hot
    /// pool needed by validated traffic).
    pub max_concurrent: usize,
}

impl Default for SpeculativeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_ms: 15_000,
            timeout_ms: 1_500,
            max_concurrent: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct PolishConfig {
    pub enabled: bool,
    /// The whole latency budget of the same-language tidy-up; past it raw text ships.
    pub timeout_ms: u64,
}

impl Default for PolishConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 1_500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct MtConfig {
    pub realtime: RealtimeConfig,
    pub chat: ChatConfig,
    pub speculative: SpeculativeConfig,
    pub polish: PolishConfig,
    /// Glossary terms sent per call, exact matches first (upstream 12).
    pub glossary_limit: usize,
    /// Context lines reaching the prompt (spec 13 section 2.3: 4).
    pub context_lines: usize,
    /// Recent source utterances kept per room as context (spec 13: 3).
    pub recent_context_segments: usize,
    /// Utterances below this average logprob never become context for later ones.
    pub context_min_confidence: f32,
    /// A segment below this opens the ASR-repair clause even without glossary/context.
    pub repair_confidence_floor: f32,
    /// A later segment of the same speaker waits at most this for the earlier one to publish,
    /// then goes out anyway; nothing is dropped (spec 08 section 5: 10 s).
    pub publish_order_wait_ms: u64,
    /// Segments translating concurrently (upstream `_MAX_IN_FLIGHT` 32): backpressure towards STT.
    pub max_in_flight: usize,
}

impl Default for MtConfig {
    fn default() -> Self {
        Self {
            realtime: RealtimeConfig::default(),
            chat: ChatConfig::default(),
            speculative: SpeculativeConfig::default(),
            polish: PolishConfig::default(),
            glossary_limit: 12,
            context_lines: 4,
            recent_context_segments: 3,
            context_min_confidence: -0.35,
            repair_confidence_floor: -0.5,
            publish_order_wait_ms: 10_000,
            max_in_flight: 32,
        }
    }
}

impl MtConfig {
    pub fn validate(&self) -> Result<(), MtError> {
        let bad = |m: &str| Err(MtError::InvalidConfig(m.into()));
        if self.max_in_flight == 0 {
            return bad("max_in_flight must be > 0");
        }
        if self.realtime.timeout_ms == 0 || self.realtime.max_output_tokens == 0 {
            return bad("realtime timeout/max_output_tokens must be > 0");
        }
        if self.realtime.enabled && self.realtime.pool_size == 0 {
            return bad("realtime.pool_size must be > 0 when enabled");
        }
        if !["minimal", "low", "medium", "high"].contains(&self.realtime.reasoning_effort.as_str())
        {
            return bad("realtime.reasoning_effort must be minimal|low|medium|high");
        }
        if self.chat.model.is_empty() || self.chat.max_tokens == 0 {
            return bad("chat.model / chat.max_tokens");
        }
        if !(0.0..=2.0).contains(&self.chat.temperature) {
            return bad("chat.temperature out of range");
        }
        if self.speculative.max_concurrent == 0 {
            return bad("speculative.max_concurrent must be > 0");
        }
        Ok(())
    }

    pub fn publish_order_wait(&self) -> Duration {
        Duration::from_millis(self.publish_order_wait_ms)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_and_validate() {
        let c = MtConfig::default();
        c.validate().unwrap();
        assert_eq!(c.realtime.model, "gpt-realtime-2.1-mini");
        assert_eq!(c.realtime.pool_size, 4);
        assert_eq!(c.realtime.timeout_ms, 2_000);
        assert_eq!(c.realtime.max_output_tokens, 128);
        assert_eq!(c.chat.model, "gpt-5.4-nano");
        assert!(c.chat.temperature.abs() < f32::EPSILON);
        assert_eq!(c.speculative.ttl_ms, 15_000);
        assert_eq!(c.publish_order_wait_ms, 10_000);
    }

    #[test]
    fn partial_json_and_rejections() {
        let c: MtConfig =
            serde_json::from_str(r#"{"realtime":{"timeout_ms":900},"glossary_limit":3}"#).unwrap();
        assert_eq!(c.realtime.timeout_ms, 900);
        assert_eq!(c.realtime.pool_size, 4);
        assert_eq!(c.glossary_limit, 3);
        let mut bad = c.clone();
        bad.realtime.reasoning_effort = "turbo".into();
        assert!(bad.validate().is_err());
        let mut bad = c;
        bad.max_in_flight = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn debug_hides_key() {
        let mut c = RealtimeConfig::default();
        c.api_key = "sk-secret".into();
        assert!(!format!("{c:?}").contains("sk-secret"));
    }
}
