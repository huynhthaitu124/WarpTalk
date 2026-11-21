//! One prosodic thread over one socket.
//!
//! A [`SpeakContext`] owns a whole connection (upstream: connection per context, so a context
//! that stalls cannot head-of-line block another). Sentences are pushed with `continue: true`
//! and `flush: true`; audio is read until the matching `flush_done`, while the context (and so
//! the prosody) carries on into the next sentence. The caller ends the context with
//! [`SpeakContext::close`] as soon as no sentence is queued, because an idle open context holds
//! a vendor concurrency slot (spec 08 section 4 item 10, upstream measurement).
//!
//! Audio is handed over chunk by chunk through an `mpsc::Sender<Vec<f32>>` as it lands: the
//! caller can start playing before the sentence has finished generating.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::error::CartesiaError;
use crate::pool::{WarmSocket, WsStream};
use crate::protocol::{
    CancelRequest, GenerationConfig, GenerationRequest, OutputFormat, Pcm16Decoder, ServerMessage,
    VoiceSpec,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakStats {
    pub samples: usize,
    pub first_audio_at: Option<Instant>,
    pub duration_ms: u32,
}

pub struct SpeakContext {
    ws: WsStream,
    context_id: String,
    model_id: String,
    voice_id: String,
    language: String,
    sample_rate: u32,
    sentence_timeout: Duration,
    close_ack: Duration,
    flushes: u64,
    closed: bool,
    seen_flush_ids: BTreeSet<u64>,
    last_activity: Instant,
}

pub(crate) struct ContextParams {
    pub model_id: String,
    pub voice_id: String,
    pub language: String,
    pub sample_rate: u32,
    pub sentence_timeout: Duration,
    pub close_ack: Duration,
}

impl SpeakContext {
    pub(crate) fn new(socket: WarmSocket, p: ContextParams) -> Self {
        Self {
            ws: socket.ws,
            context_id: uuid::Uuid::now_v7().to_string(),
            model_id: p.model_id,
            voice_id: p.voice_id,
            language: p.language,
            sample_rate: p.sample_rate,
            sentence_timeout: p.sentence_timeout,
            close_ack: p.close_ack,
            flushes: 0,
            closed: false,
            seen_flush_ids: BTreeSet::new(),
            last_activity: Instant::now(),
        }
    }

    pub fn context_id(&self) -> &str {
        &self.context_id
    }
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn sentences_spoken(&self) -> u64 {
        self.flushes
    }

    /// Whether another sentence may be pushed: not closed, and idle for less than `idle_expiry`
    /// (the vendor retires an idle context on its own and only says so with a queued `done`
    /// nobody reads until the next push; trusting a dead one costs a sentence).
    pub fn is_reusable(&self, idle_expiry: Duration) -> bool {
        !self.closed && self.last_activity.elapsed() <= idle_expiry
    }

    async fn send_json<T: serde::Serialize>(&mut self, v: &T) -> Result<(), CartesiaError> {
        let text = serde_json::to_string(v).map_err(|e| CartesiaError::Protocol(e.to_string()))?;
        self.ws
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| CartesiaError::Closed)
    }

    /// Speaks one sentence. Each PCM chunk (mono f32 at the configured rate) goes to `out` as
    /// it arrives. A failure marks the context spent: a socket that missed one flush cannot be
    /// trusted for the next.
    pub async fn speak(
        &mut self,
        text: &str,
        gen_cfg: &GenerationConfig,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, CartesiaError> {
        if self.closed {
            return Err(CartesiaError::Closed);
        }
        // Post-increment: the first flush of a context is numbered ZERO (upstream lost two
        // releases to "from 1").
        let expected = self.flushes;
        self.flushes += 1;
        let model_id = self.model_id.clone();
        let voice_id = self.voice_id.clone();
        let language = self.language.clone();
        let context_id = self.context_id.clone();
        let req = GenerationRequest {
            model_id: &model_id,
            transcript: text,
            voice: VoiceSpec {
                mode: "id",
                id: &voice_id,
            },
            language: &language,
            context_id: &context_id,
            output_format: OutputFormat {
                container: "raw",
                encoding: "pcm_s16le",
                sample_rate: self.sample_rate,
            },
            continue_: true,
            flush: true,
            generation_config: (!gen_cfg.is_empty()).then_some(gen_cfg),
        };
        let res = match self.send_json(&req).await {
            Ok(()) => {
                match tokio::time::timeout(self.sentence_timeout, self.collect(expected, out)).await
                {
                    Ok(r) => r,
                    Err(_) => Err(CartesiaError::Timeout("sentence")),
                }
            }
            Err(e) => Err(e),
        };
        match &res {
            Ok(_) => self.last_activity = Instant::now(),
            Err(e) => {
                if matches!(e, CartesiaError::Timeout(_)) {
                    warn!(
                        expected_flush = expected,
                        seen = ?self.seen_flush_ids,
                        "cartesia flush_done not seen in time"
                    );
                }
                self.closed = true;
            }
        }
        res
    }

    async fn collect(
        &mut self,
        expected: u64,
        out: &mpsc::Sender<Vec<f32>>,
    ) -> Result<SpeakStats, CartesiaError> {
        let mut dec = Pcm16Decoder::default();
        let mut samples = 0usize;
        let mut first_audio_at = None;
        loop {
            let Some(msg) = self.ws.next().await else {
                return Err(CartesiaError::Closed);
            };
            let text = match msg.map_err(|_| CartesiaError::Closed)? {
                Message::Text(t) => t,
                Message::Close(_) => return Err(CartesiaError::Closed),
                _ => continue,
            };
            let ev: ServerMessage = match serde_json::from_str(text.as_str()) {
                Ok(e) => e,
                Err(e) => {
                    debug!(error = %e, "unparseable cartesia event ignored");
                    continue;
                }
            };
            match ev {
                ServerMessage::Chunk { data, flush_id } => {
                    if let Some(id) = flush_id {
                        self.seen_flush_ids.insert(id);
                        // Chunks of an earlier flush belong to a sentence already returned.
                        if id != expected {
                            continue;
                        }
                    }
                    let bytes = B64
                        .decode(data.as_bytes())
                        .map_err(|e| CartesiaError::Protocol(format!("bad base64 audio: {e}")))?;
                    let pcm = dec.push(&bytes);
                    if pcm.is_empty() {
                        continue;
                    }
                    first_audio_at.get_or_insert_with(Instant::now);
                    samples += pcm.len();
                    // Not guarded: a consumer that went away is a cancellation, not a vendor
                    // fault, and the caller must not believe the sentence was spoken.
                    out.send(pcm).await.map_err(|_| CartesiaError::Cancelled)?;
                }
                ServerMessage::FlushDone { flush_id } => {
                    if let Some(id) = flush_id {
                        self.seen_flush_ids.insert(id);
                    }
                    if flush_id.is_none_or(|id| id == expected) {
                        break;
                    }
                }
                ServerMessage::Done {} => {
                    // The server ended the context while a sentence was being read. What
                    // arrived is what there is; nothing more is coming.
                    self.closed = true;
                    if samples == 0 {
                        // A retired idle context reads back instantly with no audio: not a
                        // success (upstream WT-874: 37 of 90 dubs were silent this way).
                        return Err(CartesiaError::Protocol(format!(
                            "context ended before any audio for flush {expected}; saw {:?}",
                            self.seen_flush_ids
                        )));
                    }
                    break;
                }
                ServerMessage::Error {
                    error,
                    message,
                    status_code,
                } => {
                    return Err(CartesiaError::Api {
                        status: status_code,
                        message: error.or(message).unwrap_or_else(|| "unknown".into()),
                    });
                }
                ServerMessage::Other => {}
            }
        }
        if samples == 0 {
            return Err(CartesiaError::Protocol(format!(
                "flush {expected} produced no audio"
            )));
        }
        let duration_ms = (samples as u64 * 1000 / u64::from(self.sample_rate.max(1))) as u32;
        Ok(SpeakStats {
            samples,
            first_audio_at,
            duration_ms,
        })
    }

    /// Ends the context and waits, briefly, for the vendor's `done`: that, not the close being
    /// sent, is when the vendor stops counting it against the plan. Bounded by `close_ack`,
    /// send included. Returns whether the end was confirmed. The socket is dropped either way.
    pub async fn close(mut self) -> bool {
        if self.closed {
            let _ = self.ws.close(None).await;
            return true;
        }
        self.closed = true;
        let (model_id, voice_id, language, context_id) = (
            self.model_id.clone(),
            self.voice_id.clone(),
            self.language.clone(),
            self.context_id.clone(),
        );
        let end = GenerationRequest {
            model_id: &model_id,
            transcript: "",
            voice: VoiceSpec {
                mode: "id",
                id: &voice_id,
            },
            language: &language,
            context_id: &context_id,
            output_format: OutputFormat {
                container: "raw",
                encoding: "pcm_s16le",
                sample_rate: self.sample_rate,
            },
            continue_: false,
            flush: false,
            generation_config: None,
        };
        let ack = tokio::time::timeout(self.close_ack, async {
            self.send_json(&end).await?;
            while let Some(Ok(msg)) = self.ws.next().await {
                if let Message::Text(t) = msg
                    && let Ok(ServerMessage::Done {} | ServerMessage::Error { .. }) =
                        serde_json::from_str::<ServerMessage>(t.as_str())
                {
                    return Ok(());
                }
            }
            Ok::<(), CartesiaError>(())
        })
        .await;
        let _ = self.ws.close(None).await;
        matches!(ack, Ok(Ok(())))
    }

    /// Drops the context without waiting for it to finish speaking (failed sentence, room
    /// ended). The vendor forgets the context the moment the cancel is sent.
    pub async fn cancel(mut self) {
        if !self.closed {
            let ctx = self.context_id.clone();
            let _ = tokio::time::timeout(
                self.close_ack,
                self.send_json(&CancelRequest {
                    context_id: &ctx,
                    cancel: true,
                }),
            )
            .await;
        }
        let _ = self.ws.close(None).await;
    }
}
