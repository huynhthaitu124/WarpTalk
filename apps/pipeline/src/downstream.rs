//! What happens to STT output. The room worker hands every [`SttSegment`] of a room to a
//! [`DownstreamStage`]; MT and TTS plug in here later (wrapping or replacing [`SttOnlyStage`])
//! without the host knowing about them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use tracing::warn;
use warptalk_kernel::RoomId;
use warptalk_pipeline_runtime::SttSegment;
use warptalk_telemetry::{Outcome, record_stage_latency, record_stage_outcome};

use crate::results::{ResultSink, stt_result_from_segment};
use crate::routes::RoutesView;
use crate::transport::MediaSession;

/// Time from segment arrival at the stage until its `stt:results` entry was written.
pub const STAGE_RESULT_PUBLISH: &str = "stt_result_publish";
/// First caption of a turn, relative to the last speech frame (0 when it beat the end of
/// speech, which is what flash mode is for; spec 08 section 9 question 1).
pub const STAGE_E2E_FIRST_CAPTION: &str = "e2e_first_caption";
/// Final (non-early) caption of a turn, relative to the last speech frame.
pub const STAGE_E2E_FINAL_CAPTION: &str = "e2e_final_caption";
/// Outcome counter label of the stt publish.
pub const STAGE_STT_PUBLISH: &str = "stt_publish";
/// Data-channel topic of caption messages.
pub const CAPTION_TOPIC: &str = "captions";

/// Everything a stage needs for one room.
pub struct DownstreamContext {
    pub room: RoomId,
    /// Publish dub audio and data messages into the room.
    pub media: Arc<dyn MediaSession>,
    /// Durable output (`stt:results`).
    pub results: Arc<dyn ResultSink>,
    /// Current audio routes (speakers, target languages); changes as the room evolves.
    pub routes: watch::Receiver<Arc<RoutesView>>,
}

/// Consumes the segments of one room until the channel closes (room shutdown), then returns.
/// Per speaker, segments arrive in `seq` order.
#[async_trait]
pub trait DownstreamStage: Send + Sync {
    async fn run(&self, ctx: DownstreamContext, segments: mpsc::Receiver<SttSegment>);
}

/// Default stage: publish STT results to the result sink and the original-language caption to
/// the room data channel. No translation or dubbing.
#[derive(Debug, Default, Clone, Copy)]
pub struct SttOnlyStage;

#[async_trait]
impl DownstreamStage for SttOnlyStage {
    async fn run(&self, ctx: DownstreamContext, mut segments: mpsc::Receiver<SttSegment>) {
        // speaker -> (utterance_seq, first caption recorded)
        let mut first: HashMap<String, (u64, bool)> = HashMap::new();
        while let Some(seg) = segments.recv().await {
            handle_segment(&ctx, &seg, &mut first).await;
        }
    }
}

async fn handle_segment(
    ctx: &DownstreamContext,
    seg: &SttSegment,
    first: &mut HashMap<String, (u64, bool)>,
) {
    let received = Instant::now();
    let Some(result) = stt_result_from_segment(
        ctx.room,
        seg,
        received,
        chrono::Utc::now().timestamp_millis(),
    ) else {
        return;
    };
    let caption = serde_json::json!({
        "type": "stt",
        "segment_id": result.segment_id,
        "meeting_id": result.meeting_id,
        "speaker_id": result.speaker_id,
        "seq": seg.seq,
        "utterance_seq": seg.utterance_seq,
        "text": result.text,
        "lang": result.language,
        "is_early": seg.is_early,
        "ts_ms": result.timestamp_ms,
    });
    // Caption and durable write are independent: neither waits for the other.
    let (stored, shown) = tokio::join!(
        async {
            let r = ctx.results.publish_stt(&result).await;
            record_stage_latency(
                STAGE_RESULT_PUBLISH,
                received.elapsed().as_secs_f64() * 1000.0,
            );
            r
        },
        ctx.media
            .publish_data(CAPTION_TOPIC, caption.to_string().into_bytes()),
    );
    let published = Instant::now();
    match stored {
        Ok(()) => record_stage_outcome(STAGE_STT_PUBLISH, Outcome::Ok),
        Err(e) => {
            warn!(room = %ctx.room, speaker = %seg.speaker, error = %e, "stt:results publish failed");
            record_stage_outcome(STAGE_STT_PUBLISH, Outcome::Error);
        }
    }
    if let Err(e) = shown {
        warn!(room = %ctx.room, speaker = %seg.speaker, error = %e, "caption publish failed");
    }

    let since_end = |end: Option<Instant>| {
        end.map_or(0.0, |e| {
            published.saturating_duration_since(e).as_secs_f64() * 1000.0
        })
    };
    let entry = first
        .entry(seg.speaker.clone())
        .or_insert((seg.utterance_seq, false));
    if entry.0 != seg.utterance_seq {
        *entry = (seg.utterance_seq, false);
    }
    if !entry.1 {
        entry.1 = true;
        record_stage_latency(STAGE_E2E_FIRST_CAPTION, since_end(seg.stamps.speech_end));
    }
    if !seg.is_early {
        record_stage_latency(STAGE_E2E_FINAL_CAPTION, since_end(seg.stamps.speech_end));
    }
}
