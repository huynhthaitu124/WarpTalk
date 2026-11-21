//! Wire types for the OpenAI Realtime **transcription** WebSocket.
//!
//! Endpoint: `wss://api.openai.com/v1/realtime?intent=transcription`, `Authorization: Bearer`.
//!
//! Reference (UNVERIFIED from this build environment: `developers.openai.com` and
//! `platform.openai.com` are not reachable from the sandbox, so event names are taken from the
//! upstream system's working integration, `warptalk-ai/stt_worker/model.py`, and cross-checked
//! against search-result excerpts of the guide):
//! <https://developers.openai.com/api/docs/guides/realtime-transcription>
//! (older URL: <https://platform.openai.com/docs/guides/realtime-transcription>).
//!
//! Unverified specifics, each marked `UNVERIFIED` below: the `session.update` payload shape for
//! `type: "transcription"`, the `logprobs` include selector, `languages` (plural) for the
//! next-generation models, the `error.code` for committing an empty buffer.
//!
//! Server events we do not model deserialize to [`ServerEvent::Other`] and are ignored.

use serde::{Deserialize, Serialize};

pub const REALTIME_SAMPLE_RATE: u32 = 24_000;
pub const LOGPROBS_INCLUDE: &str = "item.input_audio_transcription.logprobs"; // UNVERIFIED

// ---- client -> server -------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ClientEvent {
    #[serde(rename = "session.update")]
    SessionUpdate { session: SessionConfig },
    #[serde(rename = "input_audio_buffer.append")]
    Append { audio: String },
    #[serde(rename = "input_audio_buffer.commit")]
    Commit,
    #[serde(rename = "input_audio_buffer.clear")]
    Clear,
}

/// `session` object of `session.update` for a transcription session. // UNVERIFIED shape
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionConfig {
    /// Always `"transcription"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub audio: AudioConfig,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioConfig {
    pub input: InputConfig,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InputConfig {
    pub format: AudioFormat,
    pub transcription: TranscriptionConfig,
    /// Serialized as `null`: server VAD is off, we commit ourselves at `UtteranceEnd`.
    pub turn_detection: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub rate: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct TranscriptionConfig {
    pub model: String,
    /// Single language hint (older models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Plural hint list (`gpt-transcribe` / `gpt-live-transcribe`). // UNVERIFIED
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl SessionConfig {
    pub fn transcription(t: TranscriptionConfig, include_logprobs: bool) -> Self {
        Self {
            kind: "transcription",
            audio: AudioConfig {
                input: InputConfig {
                    format: AudioFormat {
                        kind: "audio/pcm",
                        rate: REALTIME_SAMPLE_RATE,
                    },
                    transcription: t,
                    turn_detection: None,
                },
            },
            include: if include_logprobs {
                vec![LOGPROBS_INCLUDE.to_string()]
            } else {
                Vec::new()
            },
        }
    }
}

// ---- server -> client -------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "session.created")]
    SessionCreated {},
    #[serde(rename = "session.updated")]
    SessionUpdated {},
    /// The commit was accepted; `item_id` names the conversation item holding its audio.
    #[serde(rename = "input_audio_buffer.committed")]
    Committed {
        item_id: String,
        #[serde(default)]
        previous_item_id: Option<String>,
    },
    /// Incremental text. Append-only within one item.
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    Delta {
        item_id: String,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    Completed {
        item_id: String,
        #[serde(default)]
        transcript: String,
        #[serde(default)]
        logprobs: Option<Vec<TokenLogprob>>,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.failed")]
    Failed {
        item_id: String,
        #[serde(default)]
        error: Option<ErrorBody>,
    },
    #[serde(rename = "error")]
    Error { error: ErrorBody },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TokenLogprob {
    #[serde(default)]
    pub token: String,
    pub logprob: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct ErrorBody {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `error.code` of committing an empty buffer. // UNVERIFIED
pub const ERR_COMMIT_EMPTY: &str = "input_audio_buffer_commit_empty";

impl ErrorBody {
    pub fn is_commit_empty(&self) -> bool {
        self.code.as_deref() == Some(ERR_COMMIT_EMPTY)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn append_commit_clear_serialize() {
        let v = serde_json::to_value(ClientEvent::Append { audio: "AAA=".into() }).unwrap();
        assert_eq!(v, json!({"type":"input_audio_buffer.append","audio":"AAA="}));
        assert_eq!(
            serde_json::to_value(ClientEvent::Commit).unwrap(),
            json!({"type":"input_audio_buffer.commit"})
        );
        assert_eq!(
            serde_json::to_value(ClientEvent::Clear).unwrap(),
            json!({"type":"input_audio_buffer.clear"})
        );
    }

    #[test]
    fn session_update_shape() {
        let cfg = SessionConfig::transcription(
            TranscriptionConfig {
                model: "gpt-live-transcribe".into(),
                languages: vec!["vi".into(), "en".into()],
                ..TranscriptionConfig::default()
            },
            false,
        );
        let v = serde_json::to_value(ClientEvent::SessionUpdate { session: cfg }).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "session.update",
                "session": {
                    "type": "transcription",
                    "audio": {"input": {
                        "format": {"type": "audio/pcm", "rate": 24000},
                        "transcription": {"model": "gpt-live-transcribe", "languages": ["vi","en"]},
                        "turn_detection": null
                    }}
                }
            })
        );
        let with_lp = SessionConfig::transcription(TranscriptionConfig::default(), true);
        let v = serde_json::to_value(with_lp).unwrap();
        assert_eq!(v["include"], json!([LOGPROBS_INCLUDE]));
    }

    #[test]
    fn parses_server_events() {
        let committed: ServerEvent = serde_json::from_str(
            r#"{"type":"input_audio_buffer.committed","event_id":"e1","item_id":"item_1","previous_item_id":null}"#,
        )
        .unwrap();
        assert_eq!(
            committed,
            ServerEvent::Committed {
                item_id: "item_1".into(),
                previous_item_id: None
            }
        );
        let delta: ServerEvent = serde_json::from_str(
            r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","content_index":0,"delta":"Hello"}"#,
        )
        .unwrap();
        assert_eq!(
            delta,
            ServerEvent::Delta {
                item_id: "item_1".into(),
                delta: "Hello".into()
            }
        );
        let done: ServerEvent = serde_json::from_str(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"item_1","content_index":0,"transcript":"Hello.","logprobs":[{"token":"Hello","logprob":-0.1,"bytes":[72]},{"token":".","logprob":-0.3,"bytes":[46]}]}"#,
        )
        .unwrap();
        let ServerEvent::Completed { transcript, logprobs, .. } = done else {
            panic!("wrong variant");
        };
        assert_eq!(transcript, "Hello.");
        assert_eq!(logprobs.unwrap().len(), 2);
        let created: ServerEvent =
            serde_json::from_str(r#"{"type":"session.created","session":{"id":"s"}}"#).unwrap();
        assert_eq!(created, ServerEvent::SessionCreated {});
    }

    #[test]
    fn completed_without_logprobs_and_unknown_events() {
        let e: ServerEvent = serde_json::from_str(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i","transcript":"x"}"#,
        )
        .unwrap();
        assert!(matches!(e, ServerEvent::Completed { logprobs: None, .. }));
        let other: ServerEvent =
            serde_json::from_str(r#"{"type":"rate_limits.updated","rate_limits":[]}"#).unwrap();
        assert_eq!(other, ServerEvent::Other);
    }

    #[test]
    fn error_event() {
        let e: ServerEvent = serde_json::from_str(
            r#"{"type":"error","error":{"type":"invalid_request_error","code":"input_audio_buffer_commit_empty","message":"empty"}}"#,
        )
        .unwrap();
        let ServerEvent::Error { error } = e else {
            panic!("wrong variant")
        };
        assert!(error.is_commit_empty());
    }
}
