//! Minimal WAV reader/writer (PCM16 and float32, any channel count mixed down to mono), so
//! simulated participants can replay recordings without a codec dependency.

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or malformed wav: {0}")]
    Format(String),
}

/// Mono samples in `[-1, 1]` and the sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct WavAudio {
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
}

pub fn read_wav_file(path: &Path) -> Result<WavAudio, WavError> {
    parse_wav(&std::fs::read(path)?)
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

pub fn parse_wav(bytes: &[u8]) -> Result<WavAudio, WavError> {
    let bad = |m: &str| WavError::Format(m.to_owned());
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32_at(bytes, pos + 4).ok_or_else(|| bad("truncated chunk"))? as usize;
        let body_start = pos + 8;
        let body_end = (body_start + len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                let tag = u16_at(body, 0).ok_or_else(|| bad("short fmt"))?;
                let ch = u16_at(body, 2).ok_or_else(|| bad("short fmt"))?;
                let rate = u32_at(body, 4).ok_or_else(|| bad("short fmt"))?;
                let bits = u16_at(body, 14).ok_or_else(|| bad("short fmt"))?;
                fmt = Some((tag, ch, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos = body_start + len + (len & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or_else(|| bad("missing fmt chunk"))?;
    let data = data.ok_or_else(|| bad("missing data chunk"))?;
    if ch == 0 || rate == 0 {
        return Err(bad("zero channels or rate"));
    }
    let ch = usize::from(ch);
    let interleaved: Vec<f32> = match (tag, bits) {
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(i16::from_le_bytes(*c)) / 32768.0)
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        _ => {
            return Err(WavError::Format(format!(
                "format tag {tag} / {bits} bits not supported (use PCM16 or float32)"
            )));
        }
    };
    let samples = interleaved
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok(WavAudio {
        samples,
        sample_rate_hz: rate,
    })
}

/// Encodes mono PCM16.
pub fn encode_wav_pcm16(samples: &[f32], sample_rate_hz: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&(sample_rate_hz * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_pcm16() {
        let src: Vec<f32> = (0..480).map(|i| (i as f32 / 480.0) - 0.5).collect();
        let wav = parse_wav(&encode_wav_pcm16(&src, 16_000)).unwrap();
        assert_eq!(wav.sample_rate_hz, 16_000);
        assert_eq!(wav.samples.len(), src.len());
        for (a, b) in wav.samples.iter().zip(&src) {
            assert!((a - b).abs() < 1e-3);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_wav(b"nope").is_err());
    }
}
