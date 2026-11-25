//! Chat configuration: settings namespace `assistant_chat` (platform > workspace > room scope).
//!
//! Every field has a serde default so partial documents are valid. Call [`register`] once so
//! `put` validates documents.
//!
//! ```json
//! { "enabled": true, "model": "gpt-4.1", "max_question_chars": 1000,
//!   "rate_limit_max": 20, "rate_limit_window_secs": 60, "recent_tokens": 6000 }
//! ```

use serde::{Deserialize, Serialize};
use warptalk_settings::{SettingsStore, Validate, check_range};

/// Settings namespace of [`ChatConfig`].
pub const NS_CHAT: &str = "assistant_chat";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// Master switch for ask and recap.
    pub enabled: bool,
    /// Model when the resolver (BYO) does not choose one.
    pub model: String,
    pub temperature: f32,
    pub max_output_tokens: u32,
    pub recap_max_output_tokens: u32,
    /// Question length cap in characters.
    pub max_question_chars: u32,
    /// Requests (ask + recap, cache hits excluded) per user per window.
    pub rate_limit_max: u32,
    pub rate_limit_window_secs: u32,
    /// Estimated tokens of the most recent transcript sent with every question.
    pub recent_tokens: u32,
    /// Estimated tokens of earlier transcript lines picked by keyword relevance.
    pub retrieved_tokens: u32,
    /// Estimated tokens of document snippets.
    pub docs_tokens: u32,
    /// Estimated tokens of earlier-meeting context.
    pub prior_tokens: u32,
    pub chars_per_token: f32,
    /// Transcript segments loaded per request (oldest first).
    pub max_segments: u32,
    /// One transcript line is cut to this many characters.
    pub max_line_chars: u32,
    /// Snippets requested from knowledge for `meeting+docs`.
    pub doc_top_k: u32,
    /// Snippets requested for `workspace` scope.
    pub workspace_top_k: u32,
    /// Earlier question/answer pairs of the user replayed to the model.
    pub history_turns: u32,
    /// Largest page of `ask/history`.
    pub history_max_page: u32,
    /// Seconds a recap stays cached per (room, window end). 0 disables the cache.
    pub recap_cache_secs: u32,
    /// Allowed `last_minutes` values of a recap (`null` = whole meeting is always allowed).
    pub recap_windows: Vec<u32>,
    pub llm_timeout_secs: u64,
    /// Answers returned without calling the LLM when nothing relevant exists.
    pub not_found_message_vi: String,
    pub not_found_message_en: String,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "gpt-4.1".into(),
            temperature: 0.2,
            max_output_tokens: 900,
            recap_max_output_tokens: 900,
            max_question_chars: 1000,
            rate_limit_max: 20,
            rate_limit_window_secs: 60,
            recent_tokens: 6000,
            retrieved_tokens: 3000,
            docs_tokens: 2500,
            prior_tokens: 1500,
            chars_per_token: 4.0,
            max_segments: 20_000,
            max_line_chars: 800,
            doc_top_k: 5,
            workspace_top_k: 10,
            history_turns: 4,
            history_max_page: 100,
            recap_cache_secs: 60,
            recap_windows: vec![5, 10, 15],
            llm_timeout_secs: 90,
            not_found_message_vi: "Chưa có trong cuộc họp/tài liệu.".into(),
            not_found_message_en: "Not in the meeting or documents yet.".into(),
        }
    }
}

impl ChatConfig {
    /// Rough token estimate of `chars` characters.
    pub fn tokens_of(&self, chars: usize) -> u32 {
        (chars as f32 / self.chars_per_token.max(0.5)).ceil() as u32
    }

    /// Character budget of `tokens` tokens.
    pub fn chars_of(&self, tokens: u32) -> usize {
        (tokens as f32 * self.chars_per_token.max(0.5)) as usize
    }
}

impl Validate for ChatConfig {
    fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty".into());
        }
        if self.not_found_message_vi.trim().is_empty()
            || self.not_found_message_en.trim().is_empty()
        {
            return Err("not_found messages must not be empty".into());
        }
        if self.recap_windows.iter().any(|m| *m == 0 || *m > 24 * 60) {
            return Err("recap_windows entries must be 1..=1440 minutes".into());
        }
        check_range("temperature", self.temperature, 0.0, 2.0)?;
        check_range("max_output_tokens", self.max_output_tokens, 50, 32_000)?;
        check_range(
            "recap_max_output_tokens",
            self.recap_max_output_tokens,
            50,
            32_000,
        )?;
        check_range("max_question_chars", self.max_question_chars, 1, 20_000)?;
        check_range("rate_limit_max", self.rate_limit_max, 1, 10_000)?;
        check_range(
            "rate_limit_window_secs",
            self.rate_limit_window_secs,
            1,
            86_400,
        )?;
        check_range("recent_tokens", self.recent_tokens, 100, 200_000)?;
        check_range("retrieved_tokens", self.retrieved_tokens, 0, 200_000)?;
        check_range("docs_tokens", self.docs_tokens, 0, 200_000)?;
        check_range("prior_tokens", self.prior_tokens, 0, 200_000)?;
        check_range("chars_per_token", self.chars_per_token, 1.0, 10.0)?;
        check_range("max_segments", self.max_segments, 1, 200_000)?;
        check_range("max_line_chars", self.max_line_chars, 50, 20_000)?;
        check_range("doc_top_k", self.doc_top_k, 0, 50)?;
        check_range("workspace_top_k", self.workspace_top_k, 0, 100)?;
        check_range("history_turns", self.history_turns, 0, 20)?;
        check_range("history_max_page", self.history_max_page, 1, 500)?;
        check_range("recap_cache_secs", self.recap_cache_secs, 0, 3600)?;
        check_range("llm_timeout_secs", self.llm_timeout_secs, 5, 900)?;
        Ok(())
    }
}

/// Register [`ChatConfig`] as the document type of [`NS_CHAT`].
pub fn register(store: &SettingsStore) {
    store.register::<ChatConfig>(NS_CHAT);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_valid_and_partial_documents_fill_gaps() {
        ChatConfig::default().validate().unwrap();
        let c: ChatConfig =
            serde_json::from_value(serde_json::json!({"rate_limit_max": 3})).unwrap();
        assert_eq!(c.rate_limit_max, 3);
        assert_eq!(c.recap_windows, vec![5, 10, 15]);
        c.validate().unwrap();
    }
}
