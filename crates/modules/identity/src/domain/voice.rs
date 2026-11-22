//! Voice enrollment rules (spec 13 section 8): sample decoding and quality checks. Pure.

pub const MIN_SAMPLE_SECONDS: f32 = 5.0;
pub const MAX_SAMPLE_SECONDS: f32 = 30.0;
pub const MIN_SAMPLE_RATE: u32 = 16_000;
pub const MAX_SAMPLE_RATE: u32 = 48_000;
pub const MIN_RMS: f32 = 0.01;
pub const MAX_CLIPPED_RATIO: f32 = 0.01;
pub const MIN_ACTIVE_RATIO: f32 = 0.3;

/// Why a sample was refused. `code()` is the API error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleError {
    Invalid(&'static str),
    RateUnsupported(u32),
    TooShort,
    TooLong,
    TooQuiet,
    TooLittleSpeech,
    Clipped,
}

impl SampleError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "VOICE_SAMPLE_INVALID",
            Self::RateUnsupported(_) => "VOICE_SAMPLE_RATE_UNSUPPORTED",
            Self::TooShort => "VOICE_SAMPLE_TOO_SHORT",
            Self::TooLong => "VOICE_SAMPLE_TOO_LONG",
            Self::TooQuiet => "VOICE_SAMPLE_TOO_QUIET",
            Self::TooLittleSpeech => "VOICE_SAMPLE_TOO_LITTLE_SPEECH",
            Self::Clipped => "VOICE_SAMPLE_CLIPPED",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Invalid(m) => format!("sample is not usable audio: {m}"),
            Self::RateUnsupported(r) => format!(
                "sample rate {r} Hz is unsupported (use {MIN_SAMPLE_RATE}-{MAX_SAMPLE_RATE} Hz)"
            ),
            Self::TooShort => format!("sample must be at least {MIN_SAMPLE_SECONDS} seconds"),
            Self::TooLong => format!("sample must be at most {MAX_SAMPLE_SECONDS} seconds"),
            Self::TooQuiet => "sample is too quiet".to_owned(),
            Self::TooLittleSpeech => "sample contains too little speech".to_owned(),
            Self::Clipped => "sample is clipped (too loud / distorted)".to_owned(),
        }
    }
}

/// Mono samples in `[-1, 1]`.
#[derive(Debug, Clone)]
pub struct Sample {
    pub rate: u32,
    pub pcm: Vec<f32>,
}

impl Sample {
    pub fn duration_seconds(&self) -> f32 {
        self.pcm.len() as f32 / self.rate as f32
    }

    /// Mono 16-bit PCM WAV (what providers accept).
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.pcm.len() * 2) as u32;
        let mut b = Vec::with_capacity(44 + self.pcm.len() * 2);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&self.rate.to_le_bytes());
        b.extend_from_slice(&(self.rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.pcm {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

fn pcm16_to_mono(bytes: &[u8], channels: usize) -> Vec<f32> {
    let frame = 2 * channels;
    bytes
        .chunks_exact(frame)
        .map(|f| {
            let sum: f32 = f
                .chunks_exact(2)
                .map(|p| f32::from(i16::from_le_bytes([p[0], p[1]])) / 32768.0)
                .sum();
            sum / channels as f32
        })
        .collect()
}

/// Raw little-endian mono 16-bit PCM.
pub fn decode_pcm16(bytes: &[u8], rate: u32) -> Result<Sample, SampleError> {
    if bytes.len() < 2 {
        return Err(SampleError::Invalid("empty"));
    }
    Ok(Sample {
        rate,
        pcm: pcm16_to_mono(bytes, 1),
    })
}

/// RIFF/WAVE, 16-bit integer PCM, mono or stereo (downmixed). The container is validated by
/// its magic bytes, not by any file name.
pub fn decode_wav(bytes: &[u8]) -> Result<Sample, SampleError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(SampleError::Invalid("not a WAV file"));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        if id == b"fmt " && body.len() >= 16 {
            fmt = Some((
                u16::from_le_bytes([body[0], body[1]]),
                u16::from_le_bytes([body[2], body[3]]),
                u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                u16::from_le_bytes([body[14], body[15]]),
            ));
        } else if id == b"data" {
            let (format, channels, rate, bits) =
                fmt.ok_or(SampleError::Invalid("data before fmt chunk"))?;
            if format != 1 || bits != 16 {
                return Err(SampleError::Invalid("only 16-bit PCM WAV is supported"));
            }
            if channels == 0 || channels > 2 {
                return Err(SampleError::Invalid("only mono or stereo is supported"));
            }
            if body.len() < 2 * usize::from(channels) {
                return Err(SampleError::Invalid("empty"));
            }
            return Ok(Sample {
                rate,
                pcm: pcm16_to_mono(body, usize::from(channels)),
            });
        }
        pos = body_start.saturating_add(len).saturating_add(len & 1);
    }
    Err(SampleError::Invalid("no audio data"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleAssessment {
    /// 0..=100.
    pub quality_score: i32,
    pub duration_seconds: f32,
    pub rms: f32,
    pub clipped_ratio: f32,
    pub active_ratio: f32,
}

/// Duration, sample rate, level (RMS), speech activity and clipping checks.
pub fn assess(sample: &Sample) -> Result<SampleAssessment, SampleError> {
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample.rate) {
        return Err(SampleError::RateUnsupported(sample.rate));
    }
    let duration = sample.duration_seconds();
    if duration < MIN_SAMPLE_SECONDS {
        return Err(SampleError::TooShort);
    }
    if duration > MAX_SAMPLE_SECONDS {
        return Err(SampleError::TooLong);
    }
    let n = sample.pcm.len() as f32;
    let rms = (sample.pcm.iter().map(|s| s * s).sum::<f32>() / n).sqrt();
    let clipped = sample.pcm.iter().filter(|s| s.abs() >= 0.999).count() as f32 / n;
    let frame = (sample.rate / 50) as usize;
    let frames: Vec<f32> = sample
        .pcm
        .chunks(frame.max(1))
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    let active = frames.iter().filter(|r| **r >= 0.01).count() as f32 / frames.len().max(1) as f32;
    if rms < MIN_RMS {
        return Err(SampleError::TooQuiet);
    }
    if clipped > MAX_CLIPPED_RATIO {
        return Err(SampleError::Clipped);
    }
    if active < MIN_ACTIVE_RATIO {
        return Err(SampleError::TooLittleSpeech);
    }
    let score = 0.5 * (active / 0.6).min(1.0)
        + 0.3 * (rms / 0.05).min(1.0)
        + 0.2 * (1.0 - clipped * 10.0).clamp(0.0, 1.0);
    Ok(SampleAssessment {
        quality_score: (score * 100.0).round() as i32,
        duration_seconds: duration,
        rms,
        clipped_ratio: clipped,
        active_ratio: active,
    })
}

/// SHA-256 hex digest as sent by the client (64 hex chars); normalised to lower case.
pub fn normalise_hash(h: &str) -> Option<String> {
    let h = h.trim();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f32, amp: f32) -> Sample {
        let n = (rate as f32 * secs) as usize;
        Sample {
            rate,
            pcm: (0..n)
                .map(|i| {
                    let env = 0.6 + 0.4 * (i as f32 / 4000.0).sin();
                    amp * env * (i as f32 * 0.07).sin()
                })
                .collect(),
        }
    }

    #[test]
    fn good_sample_passes_and_wav_round_trips() {
        let s = tone(16_000, 6.0, 0.3);
        let a = assess(&s).unwrap();
        assert!(a.quality_score > 50);
        let back = decode_wav(&s.to_wav()).unwrap();
        assert_eq!(back.rate, 16_000);
        assert_eq!(back.pcm.len(), s.pcm.len());
    }

    #[test]
    fn rejects_silent_short_long_clipped_and_bad_rate() {
        assert_eq!(assess(&tone(16_000, 6.0, 0.0)), Err(SampleError::TooQuiet));
        assert_eq!(assess(&tone(16_000, 2.0, 0.3)), Err(SampleError::TooShort));
        assert_eq!(assess(&tone(16_000, 31.0, 0.3)), Err(SampleError::TooLong));
        assert_eq!(assess(&tone(16_000, 6.0, 5.0)), Err(SampleError::Clipped));
        assert!(matches!(
            assess(&tone(8_000, 6.0, 0.3)),
            Err(SampleError::RateUnsupported(8_000))
        ));
    }

    #[test]
    fn garbage_is_not_wav() {
        assert!(decode_wav(b"hello world, not audio").is_err());
        assert_eq!(normalise_hash(&"A".repeat(64)), Some("a".repeat(64)));
        assert_eq!(normalise_hash("abc"), None);
    }
}
