//! All tunables. Every field has a default; deserialize from (partial) JSON with
//! [`CopilotConfig::from_json`]. Secrets (bearer, STT token) are never part of the config.
//!
//! `EndpointerConfig` (warptalk-ingress) is `Deserialize` only, so this config is too.

use std::collections::HashMap;

use serde::Deserialize;
use warptalk_ingress::EndpointerConfig;

use crate::error::CopilotError;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CopilotConfig {
    pub me: ChannelConfig,
    pub others: ChannelConfig,
    pub echo: EchoGuardConfig,
    pub stt: SttConfig,
    pub sink: SinkConfig,
    pub engine: EngineConfig,
}

impl CopilotConfig {
    pub fn from_json(json: &str) -> Result<Self, CopilotError> {
        let cfg: Self =
            serde_json::from_str(json).map_err(|e| CopilotError::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn channel(&self, ch: crate::Channel) -> &ChannelConfig {
        match ch {
            crate::Channel::Me => &self.me,
            crate::Channel::Others => &self.others,
        }
    }

    pub fn validate(&self) -> Result<(), CopilotError> {
        self.me.validate("me")?;
        self.others.validate("others")?;
        self.echo.validate()?;
        self.stt.validate()?;
        self.sink.validate()?;
        self.engine.validate()
    }
}

fn bad(msg: impl Into<String>) -> CopilotError {
    CopilotError::Config(msg.into())
}

/// Per-channel front end: endpointer + VAD scale + queue. Hot-reloadable
/// (`CopilotEngine::update_channel_config`); applied when the endpointer is idle.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ChannelConfig {
    pub enabled: bool,
    pub endpointer: EndpointerConfig,
    /// `EnergyVad` full-scale RMS (speech probability = rms / this, clamped).
    pub vad_full_scale_rms: f32,
    /// Added to every reported segment time (when this channel starts after session start).
    pub start_offset_ms: u64,
    /// Spoken-language hint for the STT session (ISO 639-1); `None` = auto.
    pub language_hint: Option<String>,
    /// Bounded input queue, in chunks (whatever size `push_audio` is called with).
    pub queue_capacity_chunks: usize,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpointer: EndpointerConfig::default(),
            vad_full_scale_rms: 0.05,
            start_offset_ms: 0,
            language_hint: None,
            queue_capacity_chunks: 512,
        }
    }
}

impl ChannelConfig {
    pub fn validate(&self, name: &str) -> Result<(), CopilotError> {
        self.endpointer
            .validate()
            .map_err(|e| bad(format!("{name}.endpointer: {e}")))?;
        if !(self.vad_full_scale_rms.is_finite() && self.vad_full_scale_rms > 0.0) {
            return Err(bad(format!("{name}.vad_full_scale_rms must be > 0")));
        }
        if self.queue_capacity_chunks == 0 {
            return Err(bad(format!("{name}.queue_capacity_chunks must be >= 1")));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EchoAction {
    /// Keep the segment, set `likely_echo = true` (UI can hide/dim it).
    #[default]
    Mark,
    /// Do not post the segment to the sink (still emitted as an event, flagged).
    Drop,
}

/// P0 echo guard (see `echo.rs`). Full AEC is P2.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EchoGuardConfig {
    pub enabled: bool,
    /// `others` must exceed `me` by more than this many dB (per 32 ms slice) to count as bleed.
    pub dominance_db: f32,
    /// Share of the `me` utterance's active slices that must be dominated by `others`.
    pub min_overlap_ratio: f32,
    /// Slices quieter than this RMS (full scale 1.0) are not "active".
    pub active_rms_floor: f32,
    pub action: EchoAction,
}

impl Default for EchoGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dominance_db: 10.0,
            min_overlap_ratio: 0.5,
            active_rms_floor: 0.005,
            action: EchoAction::Mark,
        }
    }
}

impl EchoGuardConfig {
    fn validate(&self) -> Result<(), CopilotError> {
        if !self.dominance_db.is_finite() || self.dominance_db < 0.0 {
            return Err(bad("echo.dominance_db must be >= 0"));
        }
        if !(0.0..=1.0).contains(&self.min_overlap_ratio) {
            return Err(bad("echo.min_overlap_ratio must be in [0, 1]"));
        }
        if !self.active_rms_floor.is_finite() || self.active_rms_floor < 0.0 {
            return Err(bad("echo.active_rms_floor must be >= 0"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SttBackend {
    /// OpenAI Realtime transcription with a server-issued ephemeral token.
    #[default]
    Realtime,
    /// Deterministic fake ("utterance N on <channel>") for tests / offline demos.
    Fake,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    pub backend: SttBackend,
    /// Used when the token response has no `websocketUrl`.
    pub default_websocket_url: String,
    pub language_hint: Option<String>,
    /// Refresh the cached token when it expires within this many seconds.
    pub token_refresh_margin_s: u64,
    /// Share one token between sessions until it nears expiry (else fetch per session).
    pub reuse_token: bool,
    pub connect_timeout_ms: u64,
    pub event_timeout_ms: u64,
    /// Max time for `open_session` (token fetch + connect) before the attempt is abandoned.
    pub open_timeout_ms: u64,
    pub min_avg_logprob: f32,
    pub min_avg_logprob_by_language: HashMap<String, f32>,
    /// `None`: ask for logprobs unless the model is known to reject them.
    pub request_logprobs: Option<bool>,
    /// Fake backend: artificial delay between commit and its segment.
    pub fake_latency_ms: u64,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            backend: SttBackend::Realtime,
            default_websocket_url: "wss://api.openai.com/v1/realtime".into(),
            language_hint: None,
            token_refresh_margin_s: 60,
            reuse_token: true,
            connect_timeout_ms: 5_000,
            event_timeout_ms: 15_000,
            open_timeout_ms: 10_000,
            min_avg_logprob: -0.7,
            min_avg_logprob_by_language: HashMap::new(),
            request_logprobs: None,
            fake_latency_ms: 0,
        }
    }
}

impl SttConfig {
    fn validate(&self) -> Result<(), CopilotError> {
        if self.connect_timeout_ms == 0 || self.event_timeout_ms == 0 || self.open_timeout_ms == 0 {
            return Err(bad("stt timeouts must be > 0"));
        }
        if url::Url::parse(&self.default_websocket_url).is_err() {
            return Err(bad("stt.default_websocket_url is not a valid URL"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SinkConfig {
    /// 1..=50 (server limit per call).
    pub batch_max: usize,
    /// Wait this long after the first pending segment for more to batch with it.
    pub batch_window_ms: u64,
    pub request_timeout_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    pub retry_multiplier: f64,
    /// Offline buffer bound; the oldest segments are dropped (and counted) beyond it.
    pub max_buffered_segments: usize,
    /// `flush()` / `close()` give up (leaving the rest buffered) after this long.
    pub flush_timeout_ms: u64,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            batch_max: 20,
            batch_window_ms: 400,
            request_timeout_ms: 10_000,
            retry_initial_ms: 500,
            retry_max_ms: 15_000,
            retry_multiplier: 2.0,
            max_buffered_segments: 5_000,
            flush_timeout_ms: 15_000,
        }
    }
}

impl SinkConfig {
    fn validate(&self) -> Result<(), CopilotError> {
        if self.batch_max == 0 || self.batch_max > 50 || self.max_buffered_segments == 0 {
            return Err(bad(
                "sink.batch_max must be in 1..=50 (server limit) and max_buffered_segments >= 1",
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(bad("sink retry: 0 < retry_initial_ms <= retry_max_ms"));
        }
        if !self.retry_multiplier.is_finite() || self.retry_multiplier < 1.0 {
            return Err(bad("sink.retry_multiplier must be >= 1"));
        }
        if self.request_timeout_ms == 0 {
            return Err(bad("sink.request_timeout_ms must be > 0"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// `Levels` event period in stream ms; 0 disables.
    pub levels_interval_ms: u64,
    pub event_capacity: usize,
    /// Segment time estimate for early sentences (the model gives no timestamps).
    pub est_ms_per_char: u64,
    /// `stop()` waits this long for in-flight transcripts.
    pub drain_timeout_ms: u64,
    /// Period of the status ticker (sink online/offline, dropped audio).
    pub status_interval_ms: u64,
    /// Close and reopen the STT session at the next utterance boundary after this age
    /// (OpenAI sessions are capped at 60 min; ephemeral tokens expire sooner).
    pub session_rotate_after_ms: u64,
    /// Wait after a failed STT open before trying again.
    pub reconnect_backoff_ms: u64,
    /// Gaps in the pushed timeline above this (e.g. after drops) shift reported times.
    pub gap_tolerance_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            levels_interval_ms: 100,
            event_capacity: 1024,
            est_ms_per_char: 65,
            drain_timeout_ms: 8_000,
            status_interval_ms: 1_000,
            session_rotate_after_ms: 25 * 60 * 1000,
            // >= 6 s keeps STT token refetches under the server limit (10/min per user).
            reconnect_backoff_ms: 6_000,
            gap_tolerance_ms: 96,
        }
    }
}

impl EngineConfig {
    fn validate(&self) -> Result<(), CopilotError> {
        if self.event_capacity == 0 {
            return Err(bad("engine.event_capacity must be >= 1"));
        }
        if self.status_interval_ms == 0 {
            return Err(bad("engine.status_interval_ms must be > 0"));
        }
        Ok(())
    }
}
