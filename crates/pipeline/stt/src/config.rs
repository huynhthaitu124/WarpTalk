use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// Models that take the plural `languages` hint. Upstream also keeps them off the logprobs
/// selector (some API versions reject the whole session update).
const NEXT_GEN_MODELS: &[&str] = &["gpt-transcribe", "gpt-live-transcribe"];

/// Configuration of [`crate::OpenAiRealtimeStt`]. Deserializable (all fields have defaults) so
/// it can sit inside `warptalk-config`; the API key comes from there, never from code.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct OpenAiSttConfig {
    pub api_key: String,
    /// WebSocket base URL without query; `?intent=transcription` is appended.
    pub base_url: String,
    pub model: String,
    /// Default spoken-language hint when the speaker declared none.
    pub language_hint: Option<String>,
    /// Pre-connected sockets kept ready (spec 08: `realtime_pool_size` 4).
    pub pool_size: usize,
    /// Completed transcripts whose average token logprob is below this are discarded.
    pub min_avg_logprob: f32,
    /// Per-language floors, keyed by base language code (`"ja"`, `"vi"`).
    pub min_avg_logprob_by_language: HashMap<String, f32>,
    pub connect_timeout_ms: u64,
    /// Max silence from the server while a commit is outstanding before it is failed.
    pub event_timeout_ms: u64,
    /// OpenAI's hard session cap is 60 minutes; sockets older than this are never handed out.
    pub max_socket_age_s: u64,
    /// A claimed socket must have at least this much life left (it will serve a whole meeting).
    pub min_socket_remaining_s: u64,
    /// `None`: request token logprobs unless the model is known to reject the selector.
    pub request_logprobs: Option<bool>,
}

impl Default for OpenAiSttConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "wss://api.openai.com/v1/realtime".into(),
            model: "gpt-live-transcribe".into(),
            language_hint: None,
            pool_size: 4,
            min_avg_logprob: -0.7,
            min_avg_logprob_by_language: HashMap::new(),
            connect_timeout_ms: 5_000,
            event_timeout_ms: 15_000,
            max_socket_age_s: 50 * 60,
            min_socket_remaining_s: 20 * 60,
            request_logprobs: None,
        }
    }
}

impl fmt::Debug for OpenAiSttConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiSttConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("pool_size", &self.pool_size)
            .finish_non_exhaustive()
    }
}

impl OpenAiSttConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    pub fn event_timeout(&self) -> Duration {
        Duration::from_millis(self.event_timeout_ms)
    }

    pub fn uses_plural_languages(&self) -> bool {
        NEXT_GEN_MODELS.contains(&self.model.as_str())
    }

    pub fn wants_logprobs(&self) -> bool {
        self.request_logprobs
            .unwrap_or_else(|| !NEXT_GEN_MODELS.contains(&self.model.as_str()))
    }

    pub fn logprob_filter(&self) -> LogprobFilter {
        LogprobFilter {
            default_floor: self.min_avg_logprob,
            by_language: self.min_avg_logprob_by_language.clone(),
        }
    }
}

/// Base language code: `"ja-JP"` -> `"ja"`.
pub fn base_language(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase()
}

/// The discard floor on a completed transcript's average token logprob.
#[derive(Debug, Clone)]
pub struct LogprobFilter {
    pub default_floor: f32,
    pub by_language: HashMap<String, f32>,
}

impl LogprobFilter {
    pub fn floor_for(&self, lang: &str) -> f32 {
        self.by_language
            .get(&base_language(lang))
            .copied()
            .unwrap_or(self.default_floor)
    }

    /// `avg_logprob == None` (model gave none) is never filtered.
    pub fn keeps(&self, lang: &str, avg_logprob: Option<f32>) -> bool {
        avg_logprob.is_none_or(|lp| lp >= self.floor_for(lang))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = OpenAiSttConfig::default();
        assert_eq!(c.model, "gpt-live-transcribe");
        assert_eq!(c.pool_size, 4);
        assert!((c.min_avg_logprob + 0.7).abs() < f32::EPSILON);
        assert!(c.uses_plural_languages());
        assert!(!c.wants_logprobs());
    }

    #[test]
    fn per_language_floor_overrides_default() {
        let mut c = OpenAiSttConfig::default();
        c.min_avg_logprob_by_language.insert("ja".into(), -1.2);
        let f = c.logprob_filter();
        assert!(f.keeps("ja-JP", Some(-1.0)));
        assert!(!f.keeps("vi", Some(-1.0)));
        assert!(f.keeps("vi", Some(-0.5)));
        assert!(f.keeps("vi", None));
    }

    #[test]
    fn debug_hides_key() {
        let c = OpenAiSttConfig::new("sk-secret");
        assert!(!format!("{c:?}").contains("sk-secret"));
    }

    #[test]
    fn deserializes_partial() {
        let c: OpenAiSttConfig =
            serde_json::from_str(r#"{"pool_size":2,"min_avg_logprob_by_language":{"ja":-1.0}}"#).unwrap();
        assert_eq!(c.pool_size, 2);
        assert_eq!(c.model, "gpt-live-transcribe");
    }
}
