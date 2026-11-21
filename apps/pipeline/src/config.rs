//! Process configuration (env) and the mapping from the `pipeline` settings namespace to the
//! runtime's [`PipelineConfig`].

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use warptalk_config::{ConfigError, optional, parse_or};
use warptalk_ingress::EndpointerConfig;
use warptalk_livekit::LiveKitConfig;
use warptalk_pipeline_runtime::PipelineConfig;
use warptalk_settings::PipelineSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Simulated,
    LiveKit,
}

impl FromStr for TransportKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "simulated" => Ok(Self::Simulated),
            "livekit" => Ok(Self::LiveKit),
            other => Err(format!(
                "unknown transport {other:?}, expected simulated|livekit"
            )),
        }
    }
}

/// Env-driven settings of the host process. Provider keys fall back to env here when no
/// settings entry exists (`OPENAI_API_KEY`).
#[derive(Clone)]
pub struct AppConfig {
    /// `PIPELINE_HOST_ID`: lease owner id (default `{HOSTNAME}-{random}`).
    pub host_id: String,
    /// `PIPELINE_LEASE_TTL_MS` (default 15000, at least 1000).
    pub lease_ttl: Duration,
    /// `PIPELINE_SCAN_INTERVAL_MS` (default 5000; 0 disables the scan).
    pub scan_interval: Option<Duration>,
    /// `MEDIA_TRANSPORT` (`livekit` default; `simulated` for local runs).
    pub transport: TransportKind,
    /// `OPENAI_API_KEY`.
    pub openai_api_key: Option<String>,
    /// `OPENAI_STT_MODEL` (optional override of the STT model).
    pub openai_stt_model: Option<String>,
    /// `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` (livekit transport only).
    pub livekit: Option<LiveKitConfig>,
    /// `SIM_PARTICIPANTS`: comma separated participant ids that speak in every simulated room.
    pub sim_participants: Vec<String>,
    /// `SIM_WAV_DIR`: `<dir>/<participant id>.wav` is replayed (looped) instead of synthetic speech.
    pub sim_wav_dir: Option<PathBuf>,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("host_id", &self.host_id)
            .field("lease_ttl", &self.lease_ttl)
            .field("scan_interval", &self.scan_interval)
            .field("transport", &self.transport)
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "***"),
            )
            .field("livekit", &self.livekit)
            .field("sim_participants", &self.sim_participants)
            .finish_non_exhaustive()
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let transport: TransportKind = parse_or("MEDIA_TRANSPORT", TransportKind::LiveKit)?;
        let lease_ms: u64 = parse_or("PIPELINE_LEASE_TTL_MS", 15_000)?;
        if lease_ms < 1_000 {
            return Err(ConfigError::Invalid {
                key: "PIPELINE_LEASE_TTL_MS".into(),
                reason: "must be at least 1000".into(),
            });
        }
        let scan_ms: u64 = parse_or("PIPELINE_SCAN_INTERVAL_MS", 5_000)?;
        let host_id = optional("PIPELINE_HOST_ID").unwrap_or_else(|| {
            let host = optional("HOSTNAME").unwrap_or_else(|| "pipeline".into());
            format!(
                "{host}-{}",
                &uuid::Uuid::now_v7().simple().to_string()[20..]
            )
        });
        let livekit = if transport == TransportKind::LiveKit {
            Some(LiveKitConfig {
                url: warptalk_config::required("LIVEKIT_URL")?,
                api_key: warptalk_config::required("LIVEKIT_API_KEY")?,
                api_secret: warptalk_config::required("LIVEKIT_API_SECRET")?,
            })
        } else {
            None
        };
        Ok(Self {
            host_id,
            lease_ttl: Duration::from_millis(lease_ms),
            scan_interval: (scan_ms > 0).then(|| Duration::from_millis(scan_ms)),
            transport,
            openai_api_key: optional("OPENAI_API_KEY"),
            openai_stt_model: optional("OPENAI_STT_MODEL"),
            livekit,
            sim_participants: optional("SIM_PARTICIPANTS")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            sim_wav_dir: optional("SIM_WAV_DIR").map(PathBuf::from),
        })
    }
}

/// `pipeline` settings namespace -> runtime config. Fields of the runtime config that the
/// namespace does not carry (queue capacities, STT mode, ...) keep their defaults.
/// `stt_min_avg_logprob` and `stream_maxlen` are consumed elsewhere (STT construction,
/// `REDIS_STREAM_MAXLEN`).
pub fn pipeline_config_from_settings(s: &PipelineSettings) -> PipelineConfig {
    PipelineConfig {
        endpointer: EndpointerConfig {
            vad_threshold: s.vad_threshold,
            vad_release_threshold: s.vad_release_threshold,
            vad_pre_speech_ms: s.vad_pre_speech_ms,
            vad_min_speech_ms: s.vad_min_speech_ms,
            vad_silence_hangover_ms: s.vad_silence_hangover_ms,
            vad_seek_hangover_ms: s.vad_seek_hangover_ms,
            vad_short_turn_hangover_ms: s.vad_short_turn_hangover_ms,
            chunk_duration_ms: s.chunk_duration_ms,
            ..EndpointerConfig::default()
        },
        flash_mode: s.flash_mode_default,
        ..PipelineConfig::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_map_to_default_runtime_config() {
        let cfg = pipeline_config_from_settings(&PipelineSettings::default());
        assert_eq!(cfg, PipelineConfig::default());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn settings_override_endpointer_and_flash() {
        let s = PipelineSettings {
            vad_silence_hangover_ms: 400,
            flash_mode_default: false,
            ..PipelineSettings::default()
        };
        let cfg = pipeline_config_from_settings(&s);
        assert_eq!(cfg.endpointer.vad_silence_hangover_ms, 400);
        assert!(!cfg.flash_mode);
    }

    #[test]
    fn transport_kind_parses() {
        assert_eq!(
            "Simulated".parse::<TransportKind>().unwrap(),
            TransportKind::Simulated
        );
        assert_eq!(
            "livekit".parse::<TransportKind>().unwrap(),
            TransportKind::LiveKit
        );
        assert!("webrtc".parse::<TransportKind>().is_err());
    }
}
