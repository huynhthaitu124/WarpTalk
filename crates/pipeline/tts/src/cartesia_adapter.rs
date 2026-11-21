//! Adapters from `warptalk-cartesia` to the TTS ports.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use warptalk_cartesia::{CartesiaClient, GenerationConfig, SpeakContext, default_voice_id};

use crate::error::TtsError;
use crate::ports::{
    CatalogVoice, CloneRequest, ContextParams, GenConfig, SpeakStats, SynthContext, Synthesizer,
    VoiceCatalogProvider, VoiceCloner,
};

pub struct CartesiaSynthesizer {
    client: Arc<CartesiaClient>,
}

impl CartesiaSynthesizer {
    pub fn new(client: Arc<CartesiaClient>) -> Self {
        Self { client }
    }
}

struct CartesiaCtx(SpeakContext);

#[async_trait]
impl Synthesizer for CartesiaSynthesizer {
    fn sample_rate(&self) -> u32 {
        self.client.sample_rate()
    }

    async fn open_context(&self, p: &ContextParams) -> Result<Box<dyn SynthContext>, TtsError> {
        let ctx = self.client.open_context(&p.voice_id, &p.language).await?;
        Ok(Box::new(CartesiaCtx(ctx)))
    }
}

#[async_trait]
impl SynthContext for CartesiaCtx {
    async fn speak(
        &mut self,
        text: &str,
        cfg: &GenConfig,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, TtsError> {
        let g = GenerationConfig {
            speed: cfg.speed,
            volume: cfg.volume,
            emotion: cfg.emotion.clone(),
        };
        let s = self.0.speak(text, &g, out).await?;
        Ok(SpeakStats {
            samples: s.samples,
            first_audio_at: s.first_audio_at,
        })
    }

    fn is_reusable(&self, idle_expiry: Duration) -> bool {
        self.0.is_reusable(idle_expiry)
    }

    async fn close(self: Box<Self>) {
        self.0.close().await;
    }

    async fn cancel(self: Box<Self>) {
        self.0.cancel().await;
    }
}

/// Clone creation behind the port: wraps the speaker's PCM in a RIFF header and posts it.
pub struct CartesiaVoiceCloner {
    client: Arc<CartesiaClient>,
}

impl CartesiaVoiceCloner {
    pub fn new(client: Arc<CartesiaClient>) -> Self {
        Self { client }
    }
}

/// 44-byte RIFF/WAVE header + 16-bit mono PCM at 16 kHz.
pub fn wav_16k_mono(pcm: &[f32]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let rate = 16_000u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    b
}

#[async_trait]
impl VoiceCloner for CartesiaVoiceCloner {
    async fn clone_voice(&self, req: &CloneRequest) -> Result<String, TtsError> {
        let wav = wav_16k_mono(&req.pcm_16k);
        Ok(self
            .client
            .clone_voice(&wav, &req.name, &req.language)
            .await?)
    }

    async fn delete_voice(&self, voice_id: &str) -> Result<(), TtsError> {
        Ok(self.client.delete_voice(voice_id).await?)
    }
}

pub struct CartesiaCatalog {
    client: Arc<CartesiaClient>,
}

impl CartesiaCatalog {
    pub fn new(client: Arc<CartesiaClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl VoiceCatalogProvider for CartesiaCatalog {
    async fn voices(&self, language: &str) -> Vec<CatalogVoice> {
        self.client
            .voices(language)
            .await
            .into_iter()
            .map(|v| CatalogVoice {
                id: v.id,
                name: v.name,
            })
            .collect()
    }

    fn default_voice(&self, language: &str) -> String {
        default_voice_id(language).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let w = wav_16k_mono(&[0.0, 1.0, -1.0]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 6);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32767);
    }
}
