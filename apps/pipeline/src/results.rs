//! Durable output of the pipeline: `stt:results` entries.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Instant;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use thiserror::Error;
use warptalk_bus::BusError;
use warptalk_contracts::events::{
    SttResult, TranslateResult, TranslationRoomSystemEvent, TtsResult,
};
use warptalk_contracts::streams::{
    self, STT_RESULTS, TRANSLATE_RESULTS, TRANSLATION_ROOM_SYSTEM_EVENTS, TTS_RESULTS,
};
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::SttSegment;

#[derive(Debug, Error)]
pub enum ResultError {
    #[error("bus error: {0}")]
    Bus(#[from] BusError),
}

#[async_trait]
pub trait ResultSink: Send + Sync {
    async fn publish_stt(&self, result: &SttResult) -> Result<(), ResultError>;

    /// `translate:results` (metadata and text of one translated sentence).
    async fn publish_translate(&self, _result: &TranslateResult) -> Result<(), ResultError> {
        Ok(())
    }

    /// `tts:results` (metadata only: the audio travels on the LiveKit track).
    async fn publish_tts(&self, _result: &TtsResult) -> Result<(), ResultError> {
        Ok(())
    }

    /// `translationRoom:system_events` (route degradation and the like).
    async fn publish_system_event(
        &self,
        _event: &TranslationRoomSystemEvent,
    ) -> Result<(), ResultError> {
        Ok(())
    }
}

/// `XADD stt:results` and the per-meeting copy `stt:results:{meetingId}` (spec 08 section 1),
/// both with `MAXLEN ~ maxlen`, issued concurrently on the multiplexed connection.
pub struct RedisResultSink {
    conn: ConnectionManager,
    maxlen: usize,
}

impl RedisResultSink {
    pub fn new(conn: ConnectionManager, maxlen: usize) -> Self {
        Self { conn, maxlen }
    }
}

#[async_trait]
impl ResultSink for RedisResultSink {
    async fn publish_stt(&self, r: &SttResult) -> Result<(), ResultError> {
        let mut global = self.conn.clone();
        let mut per_meeting = self.conn.clone();
        let copy = streams::per_meeting(STT_RESULTS, r.meeting_id);
        let (a, b) = tokio::join!(
            warptalk_bus::xadd_json(&mut global, STT_RESULTS, r, self.maxlen),
            warptalk_bus::xadd_json(&mut per_meeting, &copy, r, self.maxlen),
        );
        a?;
        b?;
        Ok(())
    }

    async fn publish_translate(&self, r: &TranslateResult) -> Result<(), ResultError> {
        self.xadd_pair(TRANSLATE_RESULTS, r, r.meeting_id).await
    }

    async fn publish_tts(&self, r: &TtsResult) -> Result<(), ResultError> {
        self.xadd_pair(TTS_RESULTS, r, r.meeting_id).await
    }

    async fn publish_system_event(
        &self,
        e: &TranslationRoomSystemEvent,
    ) -> Result<(), ResultError> {
        let mut conn = self.conn.clone();
        warptalk_bus::xadd_json(&mut conn, TRANSLATION_ROOM_SYSTEM_EVENTS, e, self.maxlen).await?;
        Ok(())
    }
}

impl RedisResultSink {
    /// Global stream entry plus the per-meeting copy, issued concurrently.
    async fn xadd_pair<T: serde::Serialize + Sync>(
        &self,
        stream: &str,
        r: &T,
        meeting: Option<RoomId>,
    ) -> Result<(), ResultError> {
        let mut global = self.conn.clone();
        match meeting {
            Some(m) => {
                let mut per_meeting = self.conn.clone();
                let copy = streams::per_meeting(stream, m);
                let (a, b) = tokio::join!(
                    warptalk_bus::xadd_json(&mut global, stream, r, self.maxlen),
                    warptalk_bus::xadd_json(&mut per_meeting, &copy, r, self.maxlen),
                );
                a?;
                b?;
            }
            None => {
                warptalk_bus::xadd_json(&mut global, stream, r, self.maxlen).await?;
            }
        }
        Ok(())
    }
}

/// Keeps results in memory (tests, bench, local runs without Redis).
#[derive(Debug, Default)]
pub struct MemoryResultSink {
    items: Mutex<Vec<(Instant, SttResult)>>,
    translations: Mutex<Vec<(Instant, TranslateResult)>>,
    tts: Mutex<Vec<(Instant, TtsResult)>>,
    system_events: Mutex<Vec<TranslationRoomSystemEvent>>,
}

impl MemoryResultSink {
    pub fn translations(&self) -> Vec<TranslateResult> {
        self.translations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, r)| r.clone())
            .collect()
    }

    pub fn tts_results(&self) -> Vec<TtsResult> {
        self.tts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, r)| r.clone())
            .collect()
    }

    pub fn system_events(&self) -> Vec<TranslationRoomSystemEvent> {
        self.system_events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn results(&self) -> Vec<SttResult> {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, r)| r.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl ResultSink for MemoryResultSink {
    async fn publish_stt(&self, r: &SttResult) -> Result<(), ResultError> {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Instant::now(), r.clone()));
        Ok(())
    }

    async fn publish_translate(&self, r: &TranslateResult) -> Result<(), ResultError> {
        self.translations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Instant::now(), r.clone()));
        Ok(())
    }

    async fn publish_tts(&self, r: &TtsResult) -> Result<(), ResultError> {
        self.tts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Instant::now(), r.clone()));
        Ok(())
    }

    async fn publish_system_event(
        &self,
        e: &TranslationRoomSystemEvent,
    ) -> Result<(), ResultError> {
        self.system_events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(e.clone());
        Ok(())
    }
}

/// Per-room facts the wire result needs besides the segment itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct SttResultMeta<'a> {
    /// Display name of the speaking participant (audio routes snapshot).
    pub speaker_name: Option<&'a str>,
    /// Epoch milliseconds of the start of the room's translation session in this process;
    /// `start_ms`/`end_ms` are offsets from it and it travels as `anchor_ms`.
    pub session_start_epoch_ms: i64,
}

/// STT confidence on the wire is a probability in `0..=1` (what the transcript stores). The
/// runtime reports the average token log-probability (`<= 0`): `exp` of it, clamped. Anything
/// that is not a finite non-positive number is unknown (`None`).
pub fn logprob_to_probability(avg_logprob: f32) -> Option<f64> {
    if !avg_logprob.is_finite() || avg_logprob > 0.0 {
        return None;
    }
    Some(f64::from(avg_logprob).exp().clamp(0.0, 1.0))
}

/// Converts a runtime segment into the wire contract. `None` for empty text. `start_ms` and
/// `end_ms` are offsets from `meta.session_start_epoch_ms` (they fit the transcript's `i32`
/// columns), reconstructed from the monotonic stamps relative to `now`; `timestamp_ms` is the
/// epoch time of publication. Every segment is final text: flash (early) sentences are
/// append-only and never re-emitted, so `is_final_chunk` is always true.
pub fn stt_result_from_segment(
    meeting: RoomId,
    seg: &SttSegment,
    now: Instant,
    now_epoch_ms: i64,
    meta: &SttResultMeta<'_>,
) -> Option<SttResult> {
    if seg.text.trim().is_empty() {
        return None;
    }
    let offset = |i: Instant| {
        (now_epoch_ms
            - now.saturating_duration_since(i).as_millis() as i64
            - meta.session_start_epoch_ms)
            .clamp(0, i64::from(i32::MAX))
    };
    let start_ms = offset(seg.stamps.audio_arrived);
    let end_ms = seg
        .stamps
        .speech_end
        .map_or_else(
            || (now_epoch_ms - meta.session_start_epoch_ms).clamp(0, i64::from(i32::MAX)),
            offset,
        )
        .max(start_ms);
    let name = meta
        .speaker_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    Some(SttResult {
        segment_id: seg.segment_id.to_string(),
        meeting_id: meeting,
        speaker_id: seg.speaker.clone(),
        text: seg.text.clone(),
        language: seg.lang.clone(),
        start_ms,
        end_ms,
        anchor_ms: Some(meta.session_start_epoch_ms),
        confidence: seg.confidence.and_then(logprob_to_probability),
        chunk_index: Some(seg.seq as i64),
        is_final_chunk: true,
        timestamp_ms: Some(now_epoch_ms),
        clean_flags: None,
        far_speaker_source: name.as_ref().map(|_| "participant".to_owned()),
        far_speaker_confidence: name.as_ref().map(|_| 1.0),
        far_speaker_name: name,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn segment_maps_to_contract() {
        let now = Instant::now();
        let mut seg = SttSegment::new("hello", "en", Some(-0.2), false);
        seg.speaker = "s1".into();
        seg.seq = 3;
        seg.stamps.audio_arrived = now - Duration::from_millis(3000);
        seg.stamps.speech_end = Some(now - Duration::from_millis(800));
        let room = RoomId::new();
        let meta = SttResultMeta {
            speaker_name: Some(" Ann "),
            session_start_epoch_ms: 900_000,
        };
        let r = stt_result_from_segment(room, &seg, now, 1_000_000, &meta).unwrap();
        assert_eq!(r.start_ms, 97_000);
        assert_eq!(r.end_ms, 99_200);
        assert_eq!(r.anchor_ms, Some(900_000));
        assert_eq!(r.timestamp_ms, Some(1_000_000));
        assert_eq!(r.chunk_index, Some(3));
        assert!(r.is_final_chunk);
        assert_eq!(r.far_speaker_name.as_deref(), Some("Ann"));
        assert_eq!(r.meeting_id, room);
        let p = r.confidence.unwrap();
        assert!((p - (-0.2f64).exp()).abs() < 1e-6, "{p}");
        seg.text = "  ".into();
        assert!(stt_result_from_segment(room, &seg, now, 1, &meta).is_none());
    }

    #[test]
    fn early_segments_are_final_and_epoch_times_fit_i32() {
        let now = Instant::now();
        let epoch = chrono::Utc::now().timestamp_millis(); // about 1.8e12
        let mut seg = SttSegment::new("flash", "en", None, true);
        seg.stamps.audio_arrived = now - Duration::from_millis(2_000);
        let meta = SttResultMeta {
            speaker_name: None,
            session_start_epoch_ms: epoch - 60_000,
        };
        let r = stt_result_from_segment(RoomId::new(), &seg, now, epoch, &meta).unwrap();
        assert!(
            r.is_final_chunk,
            "early sentences are final, append-only text"
        );
        assert_eq!(r.start_ms, 58_000);
        assert!(r.end_ms <= 60_000 && r.end_ms >= r.start_ms);
        assert_eq!(r.confidence, None);
        assert_eq!(r.far_speaker_name, None);
    }

    #[test]
    fn logprob_conversion() {
        assert_eq!(logprob_to_probability(0.0), Some(1.0));
        let p = logprob_to_probability(-0.7).unwrap();
        assert!((p - 0.4966).abs() < 1e-3);
        assert_eq!(logprob_to_probability(-1000.0), Some(0.0));
        assert_eq!(logprob_to_probability(0.5), None);
        assert_eq!(logprob_to_probability(f32::NAN), None);
    }
}
