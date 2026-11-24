//! Typed settings documents. Every field has a serde default so a stored document may be
//! partial; defaults here are the code fallbacks documented in the spec, never the only
//! source of truth (platform/workspace/room rows override them at runtime).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;
use crate::validate::{Validate, check_range};

pub const NS_PIPELINE: &str = "pipeline";
pub const NS_SUGGESTION: &str = "suggestion";
pub const NS_RAG: &str = "rag";
pub const NS_AI_PROVIDERS: &str = "ai_providers";
pub const NS_FEATURES: &str = "features";

/// Register the validators of all built-in namespaces.
pub fn register_defaults(store: &SettingsStore) {
    store.register::<PipelineSettings>(NS_PIPELINE);
    store.register::<SuggestionSettings>(NS_SUGGESTION);
    store.register::<RagSettings>(NS_RAG);
    store.register::<AiProvidersSettings>(NS_AI_PROVIDERS);
    store.register::<FeatureFlags>(NS_FEATURES);
}

/// Real-time pipeline knobs (spec 08 section 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineSettings {
    /// Platform key `meetings.flash_mode_default`.
    pub flash_mode_default: bool,
    /// Platform key `meetings.chunk_duration_ms` (2000..=15000).
    pub chunk_duration_ms: u32,
    pub vad_threshold: f32,
    pub vad_release_threshold: f32,
    pub vad_pre_speech_ms: u32,
    pub vad_min_speech_ms: u32,
    pub vad_silence_hangover_ms: u32,
    pub vad_seek_hangover_ms: u32,
    pub vad_short_turn_hangover_ms: u32,
    pub stt_min_avg_logprob: f32,
    pub tts_cache_enabled: bool,
    pub tts_cache_ttl_seconds: u32,
    pub catch_up_max_speed: f32,
    pub stream_maxlen: u32,
}

impl Default for PipelineSettings {
    fn default() -> Self {
        Self {
            flash_mode_default: true,
            chunk_duration_ms: 6000,
            vad_threshold: 0.35,
            vad_release_threshold: 0.20,
            vad_pre_speech_ms: 384,
            vad_min_speech_ms: 288,
            vad_silence_hangover_ms: 576,
            vad_seek_hangover_ms: 250,
            vad_short_turn_hangover_ms: 864,
            stt_min_avg_logprob: -0.7,
            tts_cache_enabled: true,
            tts_cache_ttl_seconds: 3600,
            catch_up_max_speed: 1.25,
            stream_maxlen: 200,
        }
    }
}

impl Validate for PipelineSettings {
    fn validate(&self) -> Result<(), String> {
        check_range("chunk_duration_ms", self.chunk_duration_ms, 2000, 15000)?;
        check_range("vad_threshold", self.vad_threshold, 0.0, 1.0)?;
        check_range(
            "vad_release_threshold",
            self.vad_release_threshold,
            0.0,
            1.0,
        )?;
        if self.vad_release_threshold > self.vad_threshold {
            return Err("vad_release_threshold must not exceed vad_threshold".into());
        }
        check_range("vad_pre_speech_ms", self.vad_pre_speech_ms, 0, 5000)?;
        check_range("vad_min_speech_ms", self.vad_min_speech_ms, 0, 5000)?;
        check_range(
            "vad_silence_hangover_ms",
            self.vad_silence_hangover_ms,
            0,
            5000,
        )?;
        check_range("vad_seek_hangover_ms", self.vad_seek_hangover_ms, 0, 5000)?;
        check_range(
            "vad_short_turn_hangover_ms",
            self.vad_short_turn_hangover_ms,
            0,
            5000,
        )?;
        check_range("stt_min_avg_logprob", self.stt_min_avg_logprob, -5.0, 0.0)?;
        check_range("catch_up_max_speed", self.catch_up_max_speed, 1.0, 2.0)?;
        check_range("stream_maxlen", self.stream_maxlen, 1, 1_000_000)
    }
}

/// One prompt category the suggestion agent may emit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionCategory {
    pub id: String,
    pub enabled: bool,
}

/// AI suggestion agent (spec 09 section 12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionSettings {
    pub enabled: bool,
    pub decide_model: String,
    pub generate_model: String,
    pub decide_max_tokens: u32,
    pub generate_max_tokens: u32,
    pub temperature: f32,
    pub request_timeout_seconds: f32,
    /// `meetings.ai_suggest.min_confidence` (0..=1).
    pub min_confidence: f32,
    /// `meetings.ai_suggest.cooldown_seconds` (0..=600).
    pub cooldown_seconds: u32,
    /// `meetings.ai_suggest.max_per_meeting` (0..=200; 0 disables).
    pub max_per_meeting: u32,
    /// `meetings.ai_suggest.min_words` (1..=20).
    pub min_words: u32,
    pub min_question_words: u32,
    /// `meetings.ai_suggest.min_stt_confidence` (-5..=0).
    pub min_stt_confidence: f32,
    pub window_size: u32,
    pub max_suggestion_chars: u32,
    pub state_ttl_seconds: u32,
    pub categories: Vec<SuggestionCategory>,
    /// Prompt templates by name (`decide`, `generate`); empty means the built-in prompt.
    pub prompts: BTreeMap<String, String>,
}

impl Default for SuggestionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            decide_model: "gpt-4o-mini".into(),
            generate_model: "gpt-4.1".into(),
            decide_max_tokens: 64,
            generate_max_tokens: 200,
            temperature: 0.2,
            request_timeout_seconds: 8.0,
            min_confidence: 0.55,
            cooldown_seconds: 20,
            max_per_meeting: 30,
            min_words: 4,
            min_question_words: 2,
            min_stt_confidence: -0.5,
            window_size: 8,
            max_suggestion_chars: 140,
            state_ttl_seconds: 14400,
            categories: Vec::new(),
            prompts: BTreeMap::new(),
        }
    }
}

impl Validate for SuggestionSettings {
    fn validate(&self) -> Result<(), String> {
        check_range("min_confidence", self.min_confidence, 0.0, 1.0)?;
        check_range("cooldown_seconds", self.cooldown_seconds, 0, 600)?;
        check_range("max_per_meeting", self.max_per_meeting, 0, 200)?;
        check_range("min_words", self.min_words, 1, 20)?;
        check_range("min_stt_confidence", self.min_stt_confidence, -5.0, 0.0)?;
        check_range("temperature", self.temperature, 0.0, 2.0)?;
        check_range(
            "request_timeout_seconds",
            self.request_timeout_seconds,
            0.5,
            60.0,
        )?;
        check_range("window_size", self.window_size, 1, 100)?;
        check_range("max_suggestion_chars", self.max_suggestion_chars, 20, 2000)?;
        if self.decide_model.trim().is_empty() || self.generate_model.trim().is_empty() {
            return Err("model names must not be empty".into());
        }
        if self.categories.iter().any(|c| c.id.trim().is_empty()) {
            return Err("category ids must not be empty".into());
        }
        Ok(())
    }
}

/// Workspace knowledge / RAG (spec 14 section 8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RagSettings {
    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub embedding_batch_size: u32,
    pub embedding_timeout_ms: u32,
    pub embedding_concurrency: u32,
    pub distance_metric: String,
    pub top_k: u32,
    pub search_timeout_seconds: f32,
    pub allowlist_limit: u32,
    pub upload_limit_mb: u32,
    pub chunk_char_limit: u32,
    pub max_facts: u32,
    pub max_fact_input_chars: u32,
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            embedding_model: "text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            embedding_batch_size: 64,
            embedding_timeout_ms: 30_000,
            embedding_concurrency: 2,
            distance_metric: "cosine".into(),
            top_k: 5,
            search_timeout_seconds: 8.0,
            allowlist_limit: 200,
            upload_limit_mb: 10,
            chunk_char_limit: 2000,
            max_facts: 12,
            max_fact_input_chars: 24_000,
        }
    }
}

impl Validate for RagSettings {
    fn validate(&self) -> Result<(), String> {
        check_range("top_k", self.top_k, 1, 100)?;
        check_range("embedding_batch_size", self.embedding_batch_size, 1, 2048)?;
        check_range("embedding_concurrency", self.embedding_concurrency, 1, 64)?;
        check_range("embedding_dimensions", self.embedding_dimensions, 1, 8192)?;
        check_range("allowlist_limit", self.allowlist_limit, 1, 500)?;
        check_range("upload_limit_mb", self.upload_limit_mb, 1, 100)?;
        check_range(
            "search_timeout_seconds",
            self.search_timeout_seconds,
            0.5,
            60.0,
        )?;
        if !["cosine", "dot", "euclid"].contains(&self.distance_metric.as_str()) {
            return Err("distance_metric must be cosine, dot or euclid".into());
        }
        Ok(())
    }
}

/// Provider and model choice for one AI function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderChoice {
    pub provider: String,
    pub model: String,
}

impl Default for ProviderChoice {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: String::new(),
        }
    }
}

/// Which provider/model serves each capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiProvidersSettings {
    pub stt: ProviderChoice,
    pub translation: ProviderChoice,
    pub tts: ProviderChoice,
    pub embedding: ProviderChoice,
    pub llm: ProviderChoice,
    pub vector_db: ProviderChoice,
}

impl Default for AiProvidersSettings {
    fn default() -> Self {
        let choice = |provider: &str, model: &str| ProviderChoice {
            provider: provider.into(),
            model: model.into(),
        };
        Self {
            stt: choice("openai", "gpt-live-transcribe"),
            translation: choice("openai", "gpt-5.4-nano"),
            tts: choice("cartesia", "sonic-3.5"),
            embedding: choice("openai", "text-embedding-3-small"),
            llm: choice("openai", "gpt-4.1"),
            vector_db: choice("qdrant", ""),
        }
    }
}

impl Validate for AiProvidersSettings {
    fn validate(&self) -> Result<(), String> {
        for (name, c) in [
            ("stt", &self.stt),
            ("translation", &self.translation),
            ("tts", &self.tts),
            ("embedding", &self.embedding),
            ("llm", &self.llm),
            ("vector_db", &self.vector_db),
        ] {
            if c.provider.trim().is_empty() {
                return Err(format!("{name}.provider must not be empty"));
            }
        }
        Ok(())
    }
}

/// Product feature flags (ADR 0001): the real-time translation product is hidden by default, the
/// meeting copilot is on. Flags are advisory for clients (they hide UI); the API does not refuse
/// translation routes of existing native rooms when a flag is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureFlags {
    /// Native translation meetings (LiveKit + pipeline).
    pub translation_meetings: bool,
    pub dubbing: bool,
    pub voice_clone: bool,
    /// External meeting bridge (translation through a LiveKit stand-in seat).
    pub external_bridge: bool,
    /// Meeting assistant copilot (sessions, transcript, suggestions).
    pub copilot: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            translation_meetings: false,
            dubbing: false,
            voice_clone: false,
            external_bridge: false,
            copilot: true,
        }
    }
}

impl Validate for FeatureFlags {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod feature_tests {
    use super::*;

    #[test]
    fn defaults_and_partial_documents() {
        let d = FeatureFlags::default();
        assert!(d.copilot && !d.translation_meetings && !d.dubbing);
        let p: FeatureFlags = serde_json::from_str(r#"{"dubbing":true}"#).unwrap();
        assert!(p.dubbing && p.copilot && !p.voice_clone);
        let j = serde_json::to_value(d).unwrap();
        assert_eq!(j["translation_meetings"], false);
        assert_eq!(j["external_bridge"], false);
    }
}
