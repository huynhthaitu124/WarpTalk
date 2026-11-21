//! Durable output of the pipeline: `stt:results` entries.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Instant;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use thiserror::Error;
use warptalk_bus::BusError;
use warptalk_contracts::events::SttResult;
use warptalk_contracts::streams::{self, STT_RESULTS};
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
}

/// Keeps results in memory (tests, bench, local runs without Redis).
#[derive(Debug, Default)]
pub struct MemoryResultSink {
    items: Mutex<Vec<(Instant, SttResult)>>,
}

impl MemoryResultSink {
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
}

/// Converts a runtime segment into the wire contract. `None` for empty text. Times are epoch
/// milliseconds reconstructed from the monotonic stamps relative to `now`.
pub fn stt_result_from_segment(
    meeting: RoomId,
    seg: &SttSegment,
    now: Instant,
    now_epoch_ms: i64,
) -> Option<SttResult> {
    if seg.text.trim().is_empty() {
        return None;
    }
    let at = |i: Instant| now_epoch_ms - now.saturating_duration_since(i).as_millis() as i64;
    let start_ms = at(seg.stamps.audio_arrived);
    let end_ms = seg.stamps.speech_end.map_or(now_epoch_ms, at).max(start_ms);
    Some(SttResult {
        segment_id: seg.segment_id.to_string(),
        meeting_id: meeting,
        speaker_id: seg.speaker.clone(),
        text: seg.text.clone(),
        language: seg.lang.clone(),
        start_ms,
        end_ms,
        anchor_ms: None,
        confidence: seg.confidence.map(f64::from),
        chunk_index: Some(seg.seq as i64),
        is_final_chunk: !seg.is_early,
        timestamp_ms: Some(now_epoch_ms),
        clean_flags: None,
        far_speaker_name: None,
        far_speaker_source: None,
        far_speaker_confidence: None,
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
        let r = stt_result_from_segment(room, &seg, now, 1_000_000).unwrap();
        assert_eq!(r.start_ms, 997_000);
        assert_eq!(r.end_ms, 999_200);
        assert_eq!(r.chunk_index, Some(3));
        assert!(r.is_final_chunk);
        assert_eq!(r.meeting_id, room);
        seg.text = "  ".into();
        assert!(stt_result_from_segment(room, &seg, now, 1).is_none());
    }
}
