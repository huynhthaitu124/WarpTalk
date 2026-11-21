//! Voice selection (spec 13 section 4), the consent gate (section 5) and the progressive
//! in-meeting clone ladder (section 6.1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, watch};
use tracing::{debug, warn};

use crate::config::{QualityConfig, TtsConfig};
use crate::ports::{
    CloneRequest, ConsentChecker, VoiceCatalogProvider, VoiceCloner, VoiceProfiles,
};
use crate::types::{VoiceInfo, VoiceMode, VoiceSource};

pub const CLONE_SAMPLE_RATE: usize = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub struct SampleQuality {
    pub score: f32,
    pub passed: bool,
    pub reason: Option<&'static str>,
}

/// Quality gate on a clone sample (thresholds from config, spec 13 section 6.1: RMS, active
/// speech ratio, clipping, active-energy variation). The web's exact scoring was not available;
/// the score here is a transparent blend of the same four measures and is only used to compare
/// rungs of one speaker against each other.
pub fn assess_sample(pcm_16k: &[f32], q: &QualityConfig) -> SampleQuality {
    let fail = |reason| SampleQuality {
        score: 0.0,
        passed: false,
        reason: Some(reason),
    };
    if pcm_16k.is_empty() {
        return fail("empty");
    }
    let n = pcm_16k.len() as f32;
    let rms = (pcm_16k.iter().map(|s| s * s).sum::<f32>() / n).sqrt();
    let clipped = pcm_16k.iter().filter(|s| s.abs() >= 0.999).count() as f32 / n;
    let frame = CLONE_SAMPLE_RATE / 50; // 20 ms
    let frame_rms: Vec<f32> = pcm_16k
        .chunks(frame)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    let active: Vec<f32> = frame_rms.iter().copied().filter(|r| *r >= 0.01).collect();
    let active_ratio = active.len() as f32 / frame_rms.len().max(1) as f32;
    let variation = if active.len() > 1 {
        let mean = active.iter().sum::<f32>() / active.len() as f32;
        let var = active.iter().map(|r| (r - mean) * (r - mean)).sum::<f32>() / active.len() as f32;
        if mean > 0.0 { var.sqrt() / mean } else { 0.0 }
    } else {
        0.0
    };
    let reason = if rms < q.min_rms {
        Some("too_quiet")
    } else if active_ratio < q.min_active_speech_ratio {
        Some("too_little_speech")
    } else if clipped > q.max_clipped_ratio {
        Some("clipped")
    } else if variation < q.min_active_energy_variation {
        Some("flat_energy")
    } else {
        None
    };
    let score = 0.4 * (active_ratio / 0.5).min(1.0)
        + 0.3 * (variation / 0.4).min(1.0)
        + 0.3 * (1.0 - clipped * 10.0).clamp(0.0, 1.0);
    SampleQuality {
        score,
        passed: reason.is_none(),
        reason,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClonedVoice {
    pub voice_id: String,
    pub quality: f32,
}

type Key = (String, String);

#[derive(Default)]
struct SpeakerClone {
    buffer: Vec<f32>,
    language: String,
    next_rung: usize,
    upgrades: u32,
    last_attempt_samples: usize,
    current: Option<ClonedVoice>,
    in_flight: bool,
    started: Option<Instant>,
    /// A rung failed and there is no clone: the route is degraded to the stock voice.
    failed: bool,
    notify: Arc<Notify>,
}

/// Accumulates each speaker's own audio and clones at rising durations. Rungs are provisional;
/// a later rung replaces the current clone only when it beats it by `upgrade_margin`.
pub struct CloneLadder {
    cfg: watch::Receiver<TtsConfig>,
    cloner: Arc<dyn VoiceCloner>,
    states: Mutex<HashMap<Key, SpeakerClone>>,
}

impl CloneLadder {
    pub fn new(cfg: watch::Receiver<TtsConfig>, cloner: Arc<dyn VoiceCloner>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            cloner,
            states: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, SpeakerClone>> {
        self.states.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Feeds the speaker's audio (mono 16 kHz). Cheap; the clone call runs in the background.
    /// Call only for speakers whose consent is active.
    pub fn push_audio(
        self: &Arc<Self>,
        room: &str,
        speaker: &str,
        language: &str,
        pcm_16k: &[f32],
    ) {
        let cfg = self.cfg.borrow().clone();
        if !cfg.clone.enabled {
            return;
        }
        let key = (room.to_string(), speaker.to_string());
        let job = {
            let mut states = self.lock();
            let st = states.entry(key.clone()).or_default();
            st.language = language.to_string();
            st.buffer.extend_from_slice(pcm_16k);
            let cap = (cfg.clone.max_buffer_seconds * CLONE_SAMPLE_RATE as f32) as usize;
            if st.buffer.len() > cap {
                let drop_n = st.buffer.len() - cap;
                st.buffer.drain(..drop_n);
                st.last_attempt_samples = st.last_attempt_samples.saturating_sub(drop_n);
            }
            let rung = st.next_rung;
            let have_current = st.current.is_some();
            let rung_allowed = rung < cfg.clone.ladder_seconds.len()
                && (!have_current || st.upgrades < cfg.clone.max_upgrades);
            let enough = rung_allowed
                && st.buffer.len() as f32
                    >= cfg.clone.ladder_seconds[rung] * CLONE_SAMPLE_RATE as f32;
            // Re-assess a rejected sample only after a second of new audio.
            let grown = st.buffer.len() >= st.last_attempt_samples + CLONE_SAMPLE_RATE;
            if st.in_flight || !enough || !grown {
                None
            } else {
                st.last_attempt_samples = st.buffer.len();
                let quality = assess_sample(&st.buffer, &cfg.clone.quality);
                if !quality.passed {
                    debug!(reason = ?quality.reason, "clone sample rejected by quality gate");
                    None
                } else {
                    st.in_flight = true;
                    st.started.get_or_insert_with(Instant::now);
                    Some((
                        Arc::<[f32]>::from(st.buffer.as_slice()),
                        quality.score,
                        st.language.clone(),
                    ))
                }
            }
        };
        if let Some((pcm, score, lang)) = job {
            let me = self.clone();
            tokio::spawn(async move { me.run_rung(key, pcm, score, lang, cfg).await });
        }
    }

    async fn run_rung(
        self: Arc<Self>,
        key: Key,
        pcm: Arc<[f32]>,
        score: f32,
        language: String,
        cfg: TtsConfig,
    ) {
        let req = CloneRequest {
            name: format!("speaker-{}", key.1),
            language,
            pcm_16k: pcm,
        };
        let res = self.cloner.clone_voice(&req).await;
        let mut delete = None;
        let notify = {
            let mut states = self.lock();
            let Some(st) = states.get_mut(&key) else {
                // Revoked while cloning: the new voice must not survive.
                drop(states);
                if let Ok(id) = res {
                    let _ = self.cloner.delete_voice(&id).await;
                }
                return;
            };
            st.in_flight = false;
            st.next_rung += 1;
            match res {
                Ok(id) => match &st.current {
                    None => {
                        st.current = Some(ClonedVoice {
                            voice_id: id,
                            quality: score,
                        });
                        st.failed = false;
                    }
                    Some(cur) => {
                        st.upgrades += 1;
                        if score >= cur.quality + cfg.clone.upgrade_margin {
                            delete = Some(cur.voice_id.clone());
                            st.current = Some(ClonedVoice {
                                voice_id: id,
                                quality: score,
                            });
                        } else {
                            delete = Some(id);
                        }
                    }
                },
                Err(e) => {
                    warn!(error = %e, "voice clone rung failed");
                    st.failed = st.current.is_none();
                }
            }
            st.notify.clone()
        };
        if let Some(id) = delete {
            let _ = self.cloner.delete_voice(&id).await;
        }
        notify.notify_waiters();
    }

    pub fn current(&self, room: &str, speaker: &str) -> Option<ClonedVoice> {
        self.lock()
            .get(&(room.to_string(), speaker.to_string()))
            .and_then(|s| s.current.clone())
    }

    /// True when a clone was attempted and failed and none exists: the route runs degraded.
    pub fn failed(&self, room: &str, speaker: &str) -> bool {
        self.lock()
            .get(&(room.to_string(), speaker.to_string()))
            .is_some_and(|s| s.failed && s.current.is_none())
    }

    /// First dub of a speaker: if a clone is being created, wait for it, but at most
    /// `first_wait_ms` from the clone's start (spec 13: 2500 ms), then the stock voice is used.
    pub async fn wait_first(
        &self,
        room: &str,
        speaker: &str,
        first_wait: Duration,
    ) -> Option<ClonedVoice> {
        let deadline = {
            let states = self.lock();
            let st = states.get(&(room.to_string(), speaker.to_string()))?;
            if st.current.is_some() {
                return st.current.clone();
            }
            if !st.in_flight {
                return None;
            }
            st.started.unwrap_or_else(Instant::now) + first_wait
        };
        loop {
            let notify = {
                let states = self.lock();
                let st = states.get(&(room.to_string(), speaker.to_string()))?;
                if st.current.is_some() || !st.in_flight {
                    return st.current.clone();
                }
                st.notify.clone()
            };
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Re-check after registering, to not miss a wake between the lock and `enable`.
            if self.current(room, speaker).is_some() {
                continue;
            }
            if tokio::time::timeout_at(deadline.into(), notified)
                .await
                .is_err()
            {
                return self.current(room, speaker);
            }
        }
    }

    /// Consent revoked or participant left: forget the clone and delete the provider voice.
    pub async fn revoke(&self, room: &str, speaker: &str) {
        let removed = self.lock().remove(&(room.to_string(), speaker.to_string()));
        if let Some(st) = removed {
            st.notify.notify_waiters();
            if let Some(c) = st.current
                && let Err(e) = self.cloner.delete_voice(&c.voice_id).await
            {
                warn!(error = %e, "clone deletion failed; left pending");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub voice: VoiceInfo,
    /// Set when a clone was expected (consent, audio) but the route runs on the stock voice.
    pub clone_degraded: Option<String>,
}

pub struct VoiceResolver {
    pub cfg: watch::Receiver<TtsConfig>,
    pub consent: Arc<dyn ConsentChecker>,
    pub ladder: Arc<CloneLadder>,
    pub profiles: Arc<dyn VoiceProfiles>,
    pub catalog: Arc<dyn VoiceCatalogProvider>,
}

fn stable_hash(s: &str) -> u64 {
    // FNV-1a: stable across runs and platforms, so un-cloned speakers keep their voice.
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

impl VoiceResolver {
    /// Stock voice: chosen dub voice, catalog voice hashed from the speaker (so un-cloned
    /// speakers sound different from each other), then the built-in default.
    pub async fn standard(&self, room: &str, speaker: &str, lang: &str) -> VoiceInfo {
        let mk = |id: String, source| VoiceInfo {
            voice_id: id,
            mode: VoiceMode::Standard,
            source,
        };
        if let Some(v) = self.profiles.chosen_dub_voice(room, speaker).await {
            return mk(v, VoiceSource::ChosenDub);
        }
        let voices = self.catalog.voices(lang).await;
        if !voices.is_empty() {
            let i = (stable_hash(speaker) % voices.len() as u64) as usize;
            return mk(voices[i].id.clone(), VoiceSource::Catalog);
        }
        mk(self.catalog.default_voice(lang), VoiceSource::Default)
    }

    /// Resolves the voice for one sentence. Consent is re-checked on EVERY call and fails
    /// closed, so a revoke stops cloned speech mid-meeting without waiting for deletion.
    pub async fn resolve(&self, room: &str, speaker: &str, lang: &str, text: &str) -> Resolved {
        let cfg = self.cfg.borrow().clone();
        let wants_clone = cfg.clone.enabled && text.chars().count() >= cfg.clone.min_clone_chars;
        if wants_clone {
            let consented = self
                .consent
                .voice_clone_consented(room, speaker)
                .await
                .unwrap_or(false);
            if consented {
                let clone = self
                    .ladder
                    .wait_first(
                        room,
                        speaker,
                        Duration::from_millis(cfg.clone.first_wait_ms),
                    )
                    .await;
                if let Some(c) = clone {
                    return Resolved {
                        voice: VoiceInfo {
                            voice_id: c.voice_id,
                            mode: VoiceMode::Cloned,
                            source: VoiceSource::InMeetingClone,
                        },
                        clone_degraded: None,
                    };
                }
                if let Some(id) = self.profiles.carried_clone(room, speaker).await {
                    return Resolved {
                        voice: VoiceInfo {
                            voice_id: id,
                            mode: VoiceMode::Cloned,
                            source: VoiceSource::CarriedClone,
                        },
                        clone_degraded: None,
                    };
                }
                let degraded = self
                    .ladder
                    .failed(room, speaker)
                    .then(|| "clone_failed".to_string());
                return Resolved {
                    voice: self.standard(room, speaker, lang).await,
                    clone_degraded: degraded,
                };
            }
        }
        Resolved {
            voice: self.standard(room, speaker, lang).await,
            clone_degraded: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speechlike(secs: f32, amp: f32) -> Vec<f32> {
        let n = (secs * CLONE_SAMPLE_RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let env = 0.5 + 0.5 * ((i as f32 / 1600.0).sin()).abs();
                amp * env * (i as f32 * 0.07).sin()
            })
            .collect()
    }

    #[test]
    fn quality_gate_thresholds() {
        let q = QualityConfig::default();
        assert!(assess_sample(&speechlike(3.0, 0.3), &q).passed);
        assert_eq!(
            assess_sample(&speechlike(3.0, 0.001), &q).reason,
            Some("too_quiet")
        );
        let mut clipped = speechlike(3.0, 0.3);
        for s in clipped.iter_mut().step_by(10) {
            *s = 1.0;
        }
        assert_eq!(assess_sample(&clipped, &q).reason, Some("clipped"));
        let mut sparse = vec![0.0; 3 * CLONE_SAMPLE_RATE];
        sparse[..1600].copy_from_slice(&speechlike(0.1, 0.5));
        assert!(!assess_sample(&sparse, &q).passed);
        assert!(!assess_sample(&[], &q).passed);
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(stable_hash("alice"), stable_hash("alice"));
        assert_ne!(stable_hash("alice"), stable_hash("bob"));
    }
}
