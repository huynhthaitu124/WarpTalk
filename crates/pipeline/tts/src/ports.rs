//! Ports of the TTS stage. The vendor, the cache, consent and voice storage are all behind
//! traits so the stage is testable without a network and the vendor is replaceable.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::error::TtsError;

/// Vendor delivery controls. `None` everywhere means "say nothing": byte-for-byte the plain call.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GenConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
}

impl GenConfig {
    pub fn is_empty(&self) -> bool {
        self.speed.is_none() && self.volume.is_none() && self.emotion.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ContextParams {
    pub language: String,
    pub voice_id: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SpeakStats {
    pub samples: usize,
    pub first_audio_at: Option<Instant>,
}

#[async_trait]
pub trait Synthesizer: Send + Sync {
    fn sample_rate(&self) -> u32;
    /// Opens a prosodic context. Cheap when a warm socket exists.
    async fn open_context(&self, p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError>;
}

#[async_trait]
pub trait SynthContext: Send {
    /// Speaks one sentence; each PCM chunk goes to `out` as it arrives (streaming).
    async fn speak(
        &mut self,
        text: &str,
        cfg: &GenConfig,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, TtsError>;
    /// Whether another sentence may be pushed (not spent, not idle past `idle_expiry`).
    fn is_reusable(&self, idle_expiry: Duration) -> bool;
    /// Ends the context and waits briefly for the vendor to confirm (it counts against the plan
    /// until then).
    async fn close(self: Box<Self>);
    async fn cancel(self: Box<Self>);
}

/// A rendered sentence. Stored as 16-bit PCM (half the size of f32, inaudible loss).
#[derive(Debug, Clone, PartialEq)]
pub struct CachedAudio {
    pub sample_rate: u32,
    pub pcm: Arc<[f32]>,
}

impl CachedAudio {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(4 + self.pcm.len() * 2);
        b.extend_from_slice(&self.sample_rate.to_le_bytes());
        for s in self.pcm.iter() {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// `None` for anything that is not a rendering: a header with no samples is a miss.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (rate, body) = bytes.split_first_chunk::<4>()?;
        let sample_rate = u32::from_le_bytes(*rate);
        if sample_rate == 0 || body.len() < 2 {
            return None;
        }
        let pcm: Vec<f32> = body
            .chunks_exact(2)
            .map(|p| f32::from(i16::from_le_bytes([p[0], p[1]])) / 32767.0)
            .collect();
        Some(Self {
            sample_rate,
            pcm: pcm.into(),
        })
    }
}

/// Best effort: a cache failure is a miss, never an error.
#[async_trait]
pub trait TtsCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<CachedAudio>;
    async fn put(&self, key: &str, audio: &CachedAudio, ttl: Duration);
}

/// Per-utterance consent re-check (spec 13 section 5). `Err` is treated as "no": fail closed.
#[async_trait]
pub trait ConsentChecker: Send + Sync {
    async fn voice_clone_consented(&self, room: &str, speaker: &str) -> Result<bool, TtsError>;
}

pub struct NoConsent;

#[async_trait]
impl ConsentChecker for NoConsent {
    async fn voice_clone_consented(&self, _room: &str, _speaker: &str) -> Result<bool, TtsError> {
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct CloneRequest {
    pub name: String,
    pub language: String,
    /// The speaker's own audio, mono 16 kHz.
    pub pcm_16k: Arc<[f32]>,
}

#[async_trait]
pub trait VoiceCloner: Send + Sync {
    /// Returns the provider voice id.
    async fn clone_voice(&self, req: &CloneRequest) -> Result<String, TtsError>;
    async fn delete_voice(&self, voice_id: &str) -> Result<(), TtsError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogVoice {
    pub id: String,
    pub name: String,
}

#[async_trait]
pub trait VoiceCatalogProvider: Send + Sync {
    /// Stock voices for a language (cached by the implementation; empty when unknown).
    async fn voices(&self, language: &str) -> Vec<CatalogVoice>;
    /// Last-resort built-in voice for a language.
    fn default_voice(&self, language: &str) -> String;
}

/// Persisted voice choices (backend voice profiles).
#[async_trait]
pub trait VoiceProfiles: Send + Sync {
    /// A clone promoted from an earlier meeting. Only used with consent.
    async fn carried_clone(&self, room: &str, speaker: &str) -> Option<String>;
    /// A stock voice the speaker picked for their own dub.
    async fn chosen_dub_voice(&self, room: &str, speaker: &str) -> Option<String>;
}

pub struct NoProfiles;

#[async_trait]
impl VoiceProfiles for NoProfiles {
    async fn carried_clone(&self, _room: &str, _speaker: &str) -> Option<String> {
        None
    }
    async fn chosen_dub_voice(&self, _room: &str, _speaker: &str) -> Option<String> {
        None
    }
}
