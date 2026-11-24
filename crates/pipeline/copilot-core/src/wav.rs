//! Minimal WAV support (RIFF PCM 8/16/24/32-bit int and 32-bit float), mono downmix. Used by
//! the CLI and tests; not meant as a general codec.

use crate::error::CopilotError;

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    /// Mono (channels averaged), `[-1, 1]`.
    pub samples: Vec<f32>,
}

fn io(msg: impl Into<String>) -> CopilotError {
    CopilotError::Io(msg.into())
}

pub fn read_wav(path: &std::path::Path) -> Result<Wav, CopilotError> {
    let bytes = std::fs::read(path).map_err(|e| io(format!("{}: {e}", path.display())))?;
    parse_wav(&bytes)
}

pub fn parse_wav(b: &[u8]) -> Result<Wav, CopilotError> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(io("not a RIFF/WAVE file"));
    }
    let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(pos + 4).ok_or_else(|| io("truncated chunk"))? as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(b.len());
        let body = &b[body_start..body_end];
        match id {
            b"fmt " => {
                let tag = u16_at(body_start).ok_or_else(|| io("short fmt"))?;
                let ch = u16_at(body_start + 2).ok_or_else(|| io("short fmt"))?;
                let rate = u32_at(body_start + 4).ok_or_else(|| io("short fmt"))?;
                let bits = u16_at(body_start + 14).ok_or_else(|| io("short fmt"))?;
                // WAVE_FORMAT_EXTENSIBLE: real tag is the first two bytes of the sub-format.
                let tag = if tag == 0xFFFE {
                    u16_at(body_start + 24).unwrap_or(tag)
                } else {
                    tag
                };
                fmt = Some((tag, ch, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos = body_start + size + (size & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or_else(|| io("missing fmt chunk"))?;
    let data = data.ok_or_else(|| io("missing data chunk"))?;
    if ch == 0 || rate == 0 {
        return Err(io("bad fmt chunk"));
    }
    let bytes_per = usize::from(bits / 8);
    let sample = |s: &[u8]| -> Result<f32, CopilotError> {
        Ok(match (tag, bits) {
            (1, 8) => (f32::from(s[0]) - 128.0) / 128.0,
            (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            _ => return Err(io(format!("unsupported WAV format tag {tag} / {bits} bit"))),
        })
    };
    if bytes_per == 0 {
        return Err(io("bad bit depth"));
    }
    let frame_bytes = bytes_per * usize::from(ch);
    let mut samples = Vec::with_capacity(data.len() / frame_bytes);
    for frame in data.chunks_exact(frame_bytes) {
        let mut acc = 0.0;
        for c in frame.chunks_exact(bytes_per) {
            acc += sample(c)?;
        }
        samples.push(acc / f32::from(ch));
    }
    Ok(Wav {
        sample_rate: rate,
        samples,
    })
}

/// Encodes mono 16-bit PCM.
pub fn encode_wav_i16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let pcm = warptalk_audio::f32_to_i16(samples);
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_errors() {
        let src: Vec<f32> = (0..480).map(|i| (i as f32 / 480.0) - 0.5).collect();
        let w = parse_wav(&encode_wav_i16(&src, 48_000)).unwrap();
        assert_eq!(w.sample_rate, 48_000);
        assert_eq!(w.samples.len(), 480);
        for (a, b) in src.iter().zip(&w.samples) {
            assert!((a - b).abs() < 1e-3);
        }
        assert!(parse_wav(b"nope").is_err());
        assert!(parse_wav(&encode_wav_i16(&src, 48_000)[..30]).is_err());
    }
}
