//! Wire types of the Cartesia TTS WebSocket.
//!
//! Endpoint `wss://api.cartesia.ai/tts/websocket`, auth by `X-API-Key` header plus
//! `Cartesia-Version`. Reference: <https://docs.cartesia.ai/api-reference/tts/tts> (UNVERIFIED:
//! the docs host is not reachable from the build sandbox). Shapes below come from the upstream
//! system's working integration (`warptalk-ai/tts_worker/synthesizer.py`, `prosody_context.py`,
//! which drives the vendor SDK), so each field name that the SDK hides behind a kwarg is marked.
//!
//! Context semantics (from upstream, measured against the live API there):
//! - one `context_id` is one prosodic thread; `continue: true` keeps it open;
//! - `flush: true` puts a numbered boundary in the audio stream; ids count from ZERO per
//!   context; every `chunk` carries the `flush_id` it belongs to and `flush_done` closes it;
//! - an empty transcript with `continue: false` ends the context; the vendor answers `done`;
//! - an open context occupies a concurrency slot, idle or not (upstream measurement).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VoiceSpec<'a> {
    pub mode: &'static str,
    pub id: &'a str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutputFormat {
    pub container: &'static str,
    pub encoding: &'static str,
    pub sample_rate: u32,
}

/// Delivery controls (`generation_config`, upstream: speed in [0.6, 1.5], volume, emotion).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
}

impl GenerationConfig {
    pub fn is_empty(&self) -> bool {
        self.speed.is_none() && self.volume.is_none() && self.emotion.is_none()
    }
}

/// `generate` request. `flush` is the SDK's `flush=True` push kwarg. // UNVERIFIED field name
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GenerationRequest<'a> {
    pub model_id: &'a str,
    pub transcript: &'a str,
    pub voice: VoiceSpec<'a>,
    pub language: &'a str,
    pub context_id: &'a str,
    pub output_format: OutputFormat,
    #[serde(rename = "continue")]
    pub continue_: bool,
    pub flush: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<&'a GenerationConfig>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CancelRequest<'a> {
    pub context_id: &'a str,
    pub cancel: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Audio: base64 raw `pcm_s16le`.
    #[serde(rename = "chunk")]
    Chunk {
        data: String,
        #[serde(default)]
        flush_id: Option<u64>,
    },
    #[serde(rename = "flush_done")]
    FlushDone {
        #[serde(default)]
        flush_id: Option<u64>,
    },
    #[serde(rename = "done")]
    Done {},
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        status_code: Option<u16>,
    },
    #[serde(other)]
    Other,
}

/// Raw little-endian s16 bytes to f32, carrying an odd trailing byte across chunks.
#[derive(Debug, Default)]
pub struct Pcm16Decoder {
    carry: Option<u8>,
}

impl Pcm16Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<f32> {
        let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
        let mut it = bytes.iter().copied();
        if let Some(lo) = self.carry.take() {
            match it.next() {
                Some(hi) => out.push(f32::from(i16::from_le_bytes([lo, hi])) / 32768.0),
                None => {
                    self.carry = Some(lo);
                    return out;
                }
            }
        }
        let rest: Vec<u8> = it.collect();
        let mut pairs = rest.chunks_exact(2);
        for p in &mut pairs {
            out.push(f32::from(i16::from_le_bytes([p[0], p[1]])) / 32768.0);
        }
        if let [b] = pairs.remainder() {
            self.carry = Some(*b);
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_shape() {
        let g = GenerationConfig {
            speed: Some(1.1),
            ..GenerationConfig::default()
        };
        let r = GenerationRequest {
            model_id: "sonic-3.5",
            transcript: "hi",
            voice: VoiceSpec { mode: "id", id: "v" },
            language: "en",
            context_id: "c",
            output_format: OutputFormat {
                container: "raw",
                encoding: "pcm_s16le",
                sample_rate: 44100,
            },
            continue_: true,
            flush: true,
            generation_config: Some(&g),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["generation_config"], json!({"speed": 1.1f32}));
        assert_eq!(v["voice"], json!({"mode":"id","id":"v"}));
    }

    #[test]
    fn parses_events_and_unknown() {
        let c: ServerMessage =
            serde_json::from_str(r#"{"type":"chunk","data":"AAA=","flush_id":0,"step_time":1.0}"#)
                .unwrap();
        assert!(matches!(c, ServerMessage::Chunk { flush_id: Some(0), .. }));
        let o: ServerMessage = serde_json::from_str(r#"{"type":"timestamps"}"#).unwrap();
        assert_eq!(o, ServerMessage::Other);
        let e: ServerMessage =
            serde_json::from_str(r#"{"type":"error","error":"bad voice","status_code":404}"#)
                .unwrap();
        assert!(matches!(e, ServerMessage::Error { status_code: Some(404), .. }));
    }

    #[test]
    fn decoder_handles_split_samples() {
        let mut d = Pcm16Decoder::default();
        let a = d.push(&[0x00, 0x40, 0x01]);
        assert_eq!(a, vec![0.5]);
        let b = d.push(&[0x40]);
        assert_eq!(b.len(), 1);
        assert!((b[0] - f32::from(0x4001i16) / 32768.0).abs() < 1e-6);
        assert!(d.push(&[]).is_empty());
    }
}
