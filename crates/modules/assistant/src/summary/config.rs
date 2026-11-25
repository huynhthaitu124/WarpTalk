//! Summary configuration: settings namespace `summary` (platform > workspace > room scope).
//!
//! Every field has a serde default so partial documents are valid; prompts are versioned and an
//! override replaces the built-in text. Call [`register`] once so `put` validates documents.
//!
//! ```json
//! { "enabled": true, "model": "gpt-4.1", "prompt_version": "v1", "language": "vi",
//!   "system_prompt": null, "map_prompt": null, "reduce_prompt": null,
//!   "window_tokens": 6000, "max_segments": 2000,
//!   "memory_enabled": true, "memory_max_open_items": 10, "memory_max_summaries": 3 }
//! ```

use serde::{Deserialize, Serialize};
use warptalk_settings::{SettingsStore, Validate, check_range};

/// Settings namespace of [`SummaryConfig`].
pub const NS_SUMMARY: &str = "summary";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SummaryConfig {
    /// Master switch: when false the marker enqueues nothing (explicit regenerate still works).
    pub enabled: bool,
    /// Model when the resolver (BYO) does not choose one.
    pub model: String,
    /// Label stored with each summary; bump it together with a prompt override.
    pub prompt_version: String,
    /// Output language (ISO code or name). Empty: the room's source language.
    pub language: String,
    /// Overrides of the built-in prompts (`None` = built-in).
    pub system_prompt: Option<String>,
    pub map_prompt: Option<String>,
    pub reduce_prompt: Option<String>,
    /// Estimated tokens of transcript per map window (single pass when it all fits).
    pub window_tokens: u32,
    /// Characters per token used for the estimate.
    pub chars_per_token: f32,
    /// Segment cap per job (spec LI-12); later segments are dropped and flagged in `meta`.
    pub max_segments: u32,
    pub max_output_tokens: u32,
    pub temperature: f32,
    /// Windows summarized concurrently.
    pub map_concurrency: u32,
    /// Live follow-ups included as hints (most recent first, capped).
    pub max_followups: u32,
    /// LLM attempts per call (transient errors and unparseable replies).
    pub max_attempts: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    pub llm_timeout_secs: u64,
    /// How long a job waits for the transcript to be finalized before reading what is there.
    pub transcript_wait_secs: u64,
    /// Poll interval while waiting for the transcript to settle.
    pub transcript_poll_ms: u64,
    /// A `running` job untouched for this long is considered dead and taken over.
    pub stale_running_secs: u64,
    /// Job attempts (process restarts included) before the job is dead-lettered.
    pub max_job_attempts: u32,
    /// Feed the host's earlier open items and summaries ("Bối cảnh trước") into summaries.
    pub memory_enabled: bool,
    /// Open action items of earlier meetings in the prior context (N, newest first).
    pub memory_max_open_items: u32,
    /// Earlier meeting summaries in the prior context (K).
    pub memory_max_summaries: u32,
    /// Characters kept per prior line (task, summary).
    pub memory_line_chars: u32,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "gpt-4.1".into(),
            prompt_version: "v1".into(),
            language: String::new(),
            system_prompt: None,
            map_prompt: None,
            reduce_prompt: None,
            window_tokens: 6000,
            chars_per_token: 4.0,
            max_segments: 2000,
            max_output_tokens: 2500,
            temperature: 0.2,
            map_concurrency: 4,
            max_followups: 40,
            max_attempts: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 20_000,
            llm_timeout_secs: 120,
            transcript_wait_secs: 15,
            transcript_poll_ms: 500,
            stale_running_secs: 600,
            max_job_attempts: 3,
            memory_enabled: true,
            memory_max_open_items: 10,
            memory_max_summaries: 3,
            memory_line_chars: 300,
        }
    }
}

impl Validate for SummaryConfig {
    fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() || self.prompt_version.trim().is_empty() {
            return Err("model and prompt_version must not be empty".into());
        }
        for (name, p) in [
            ("system_prompt", &self.system_prompt),
            ("map_prompt", &self.map_prompt),
            ("reduce_prompt", &self.reduce_prompt),
        ] {
            if p.as_deref().is_some_and(|t| t.trim().is_empty()) {
                return Err(format!(
                    "{name} must not be blank (use null for the default)"
                ));
            }
        }
        check_range("window_tokens", self.window_tokens, 500, 200_000)?;
        check_range("chars_per_token", self.chars_per_token, 1.0, 10.0)?;
        check_range("max_segments", self.max_segments, 1, 100_000)?;
        check_range("max_output_tokens", self.max_output_tokens, 200, 32_000)?;
        check_range("temperature", self.temperature, 0.0, 2.0)?;
        check_range("map_concurrency", self.map_concurrency, 1, 32)?;
        check_range("max_followups", self.max_followups, 0, 500)?;
        check_range("max_attempts", self.max_attempts, 1, 10)?;
        check_range("backoff_base_ms", self.backoff_base_ms, 0, 60_000)?;
        check_range("backoff_max_ms", self.backoff_max_ms, 0, 300_000)?;
        check_range("llm_timeout_secs", self.llm_timeout_secs, 5, 900)?;
        check_range("transcript_wait_secs", self.transcript_wait_secs, 0, 300)?;
        check_range("transcript_poll_ms", self.transcript_poll_ms, 10, 10_000)?;
        check_range("stale_running_secs", self.stale_running_secs, 30, 86_400)?;
        check_range("max_job_attempts", self.max_job_attempts, 1, 10)?;
        check_range("memory_max_open_items", self.memory_max_open_items, 0, 100)?;
        check_range("memory_max_summaries", self.memory_max_summaries, 0, 20)?;
        check_range("memory_line_chars", self.memory_line_chars, 50, 2000)?;
        Ok(())
    }
}

/// Register [`SummaryConfig`] as the document type of [`NS_SUMMARY`].
pub fn register(store: &SettingsStore) {
    store.register::<SummaryConfig>(NS_SUMMARY);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_partial_documents_fill_gaps() {
        SummaryConfig::default().validate().unwrap();
        let c: SummaryConfig =
            serde_json::from_value(serde_json::json!({"language": "vi"})).unwrap();
        assert_eq!(c.language, "vi");
        assert_eq!(c.max_segments, 2000);
    }

    #[test]
    fn rejects_bad_values() {
        let c = SummaryConfig {
            window_tokens: 1,
            ..Default::default()
        };
        assert!(c.validate().is_err());
        let c = SummaryConfig {
            map_prompt: Some("  ".into()),
            ..Default::default()
        };
        assert!(c.validate().is_err());
    }
}
