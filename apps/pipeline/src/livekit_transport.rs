//! LiveKit implementation of [`MediaTransport`] (feature `livekit-rtc`, NOT compiled in the
//! default build or in this repository's CI: the `livekit` crate needs webrtc-sys, which needs
//! clang >= 21). Written against `livekit` 0.12 / `libwebrtc` 0.4.2 (read from their sources);
//! calls that could not be checked against a compiler are marked `// UNVERIFIED`.
//!
//! Mapping:
//! - join: hidden agent token from `warptalk_livekit::pipeline_agent_token`, `Room::connect`.
//!   LiveKit room name = translation room id (UNVERIFIED assumption: the meeting service
//!   creates the LiveKit room under that name) and participant identity = participant id
//!   (the audio routes reference participants by id).
//! - audio in: every subscribed remote microphone track is read with `NativeAudioStream` at
//!   16 kHz mono (native resampling, spec 08 hop 1); blocks are forwarded with `try_send`, so a
//!   slow pipeline drops audio instead of blocking the SDK thread.
//! - dub out: one `NativeAudioSource` + `LocalAudioTrack` per target language, published as
//!   `ai-interpreter-{lang}`; chunks are resampled to 48 kHz and captured in 10 ms frames.
//! - captions: reliable data packets with a topic.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use warptalk_audio::{Frame, f32_to_i16, resample};
use warptalk_kernel::RoomId;
use warptalk_livekit::{LiveKitConfig, pipeline_agent_token};

use crate::transport::{
    MEDIA_EVENTS_DROPPED_METRIC, MediaError, MediaEvent, MediaSession, MediaTransport,
    ParticipantAudio, PcmChunk,
};

/// Rate of the published dub tracks.
const PUBLISH_RATE_HZ: u32 = 48_000;
/// 10 ms at the publish rate; `NativeAudioSource::capture_frame` works in 10 ms units.
const PUBLISH_FRAME_SAMPLES: usize = 480;
/// Rate requested from `NativeAudioStream` (the pipeline consumes 16 kHz mono).
const SUBSCRIBE_RATE_HZ: i32 = 16_000;
/// Identity prefix of agents (ours and the legacy bots); their tracks are never transcribed.
const AGENT_PREFIXES: [&str; 2] = ["pipeline-agent", "ai-interpreter-"];

pub struct LiveKitTransport {
    cfg: LiveKitConfig,
    token_ttl: Duration,
}

impl LiveKitTransport {
    pub fn new(cfg: LiveKitConfig) -> Self {
        Self {
            cfg,
            token_ttl: Duration::from_secs(6 * 3600),
        }
    }
}

#[async_trait]
impl MediaTransport for LiveKitTransport {
    async fn join(&self, room: RoomId, identity: &str) -> Result<Box<dyn MediaSession>, MediaError> {
        let token = pipeline_agent_token(&self.cfg, &room.to_string(), identity, self.token_ttl)
            .map_err(|e| MediaError::Join(e.to_string()))?;
        let (lk_room, mut lk_events) = Room::connect(&self.cfg.url, &token, RoomOptions::default())
            .await
            .map_err(|e| MediaError::Join(e.to_string()))?;
        let lk_room = Arc::new(lk_room);
        info!(%room, identity, "joined LiveKit room");

        let (tx, rx) = mpsc::channel::<MediaEvent>(4096);
        let readers: Arc<Mutex<HashMap<String, JoinHandle<()>>>> = Arc::default();

        // Participants already present: `Connected` also lists them, but `remote_participants()`
        // is the simplest source. UNVERIFIED: the events of existing tracks are delivered on
        // `lk_events` after `connect` returns (TrackSubscribed), so no track handling here.
        for id in lk_room.remote_participants().keys() {
            let _ = tx.try_send(MediaEvent::ParticipantJoined {
                participant: id.to_string(),
                at: Instant::now(),
            });
        }

        let pump = {
            let tx = tx.clone();
            let readers = readers.clone();
            tokio::spawn(async move {
                while let Some(ev) = lk_events.recv().await {
                    match ev {
                        RoomEvent::ParticipantConnected(p) => {
                            let _ = tx.try_send(MediaEvent::ParticipantJoined {
                                participant: p.identity().to_string(),
                                at: Instant::now(),
                            });
                        }
                        RoomEvent::ParticipantDisconnected(p) => {
                            let _ = tx.try_send(MediaEvent::ParticipantLeft {
                                participant: p.identity().to_string(),
                                at: Instant::now(),
                            });
                        }
                        RoomEvent::TrackSubscribed { track, publication, participant } => {
                            let id = participant.identity().to_string();
                            let RemoteTrack::Audio(audio) = track else { continue };
                            if AGENT_PREFIXES.iter().any(|p| id.starts_with(p))
                                || publication.source() != TrackSource::Microphone
                            {
                                continue;
                            }
                            let sid = publication.sid().to_string();
                            let handle = tokio::spawn(read_track(audio, id, tx.clone()));
                            if let Some(old) = readers
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .insert(sid, handle)
                            {
                                old.abort();
                            }
                        }
                        RoomEvent::TrackUnsubscribed { publication, .. } => {
                            if let Some(h) = readers
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .remove(&publication.sid().to_string())
                            {
                                h.abort();
                            }
                        }
                        RoomEvent::Disconnected { reason } => {
                            warn!(?reason, "LiveKit room disconnected");
                            break;
                        }
                        _ => {}
                    }
                }
                debug!("LiveKit event pump ended");
            })
        };

        Ok(Box::new(LiveKitSession {
            room: lk_room,
            events: Mutex::new(Some(rx)),
            _tx: tx,
            dub_tracks: tokio::sync::Mutex::new(HashMap::new()),
            pump,
            readers,
        }))
    }
}

/// Pulls 16 kHz mono blocks from one remote track until it ends.
async fn read_track(track: RemoteAudioTrack, participant: String, tx: mpsc::Sender<MediaEvent>) {
    // UNVERIFIED: `queue_size_frames: None` selects the default queue size.
    let mut stream = NativeAudioStream::new(track.rtc_track(), SUBSCRIBE_RATE_HZ, 1, None);
    while let Some(frame) = stream.next().await {
        let samples: Vec<f32> = frame.data.iter().map(|s| f32::from(*s) / 32768.0).collect();
        let ev = MediaEvent::Audio(ParticipantAudio {
            participant: participant.clone(),
            frame: Frame::new(samples, frame.sample_rate),
            captured_at: Instant::now(),
        });
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(ev) {
            metrics::counter!(MEDIA_EVENTS_DROPPED_METRIC).increment(1);
        }
    }
}

struct DubTrack {
    source: NativeAudioSource,
    /// Keeps the track (and publication) alive.
    _track: LocalAudioTrack,
}

struct LiveKitSession {
    room: Arc<Room>,
    events: Mutex<Option<mpsc::Receiver<MediaEvent>>>,
    /// Keeps the event channel open while no reader task holds a sender.
    _tx: mpsc::Sender<MediaEvent>,
    dub_tracks: tokio::sync::Mutex<HashMap<String, DubTrack>>,
    pump: JoinHandle<()>,
    readers: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

#[async_trait]
impl MediaSession for LiveKitSession {
    fn take_events(&self) -> Option<mpsc::Receiver<MediaEvent>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    async fn publish_dub(
        &self,
        target_lang: &str,
        _voice: &str,
        chunk: PcmChunk,
    ) -> Result<(), MediaError> {
        let source = {
            let mut tracks = self.dub_tracks.lock().await;
            if !tracks.contains_key(target_lang) {
                // UNVERIFIED: `queue_size_ms` 1000 = look-ahead of the original bot (spec 08 2.4).
                let source =
                    NativeAudioSource::new(AudioSourceOptions::default(), PUBLISH_RATE_HZ, 1, 1000);
                let track = LocalAudioTrack::create_audio_track(
                    &format!("ai-interpreter-{target_lang}"),
                    RtcAudioSource::Native(source.clone()),
                );
                self.room
                    .local_participant()
                    .publish_track(
                        LocalTrack::Audio(track.clone()),
                        TrackPublishOptions {
                            source: TrackSource::Microphone,
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|e| MediaError::Publish(e.to_string()))?;
                tracks.insert(
                    target_lang.to_owned(),
                    DubTrack {
                        source,
                        _track: track,
                    },
                );
            }
            tracks.get(target_lang).map(|t| t.source.clone())
        }
        .ok_or(MediaError::Closed)?;

        let pcm = f32_to_i16(&resample(&chunk.samples, chunk.sample_rate_hz, PUBLISH_RATE_HZ));
        for block in pcm.chunks(PUBLISH_FRAME_SAMPLES) {
            let mut data = block.to_vec();
            data.resize(PUBLISH_FRAME_SAMPLES, 0);
            source
                .capture_frame(&AudioFrame {
                    data: Cow::Owned(data),
                    sample_rate: PUBLISH_RATE_HZ,
                    num_channels: 1,
                    samples_per_channel: PUBLISH_FRAME_SAMPLES as u32,
                })
                .await
                .map_err(|e| MediaError::Publish(e.to_string()))?;
        }
        Ok(())
    }

    async fn publish_data(&self, topic: &str, payload: Vec<u8>) -> Result<(), MediaError> {
        self.room
            .local_participant()
            .publish_data(DataPacket {
                payload,
                topic: Some(topic.to_owned()),
                reliable: true,
                destination_identities: Vec::new(),
            })
            .await
            .map_err(|e| MediaError::Publish(e.to_string()))
    }

    async fn close(&self) {
        self.pump.abort();
        for (_, h) in self.readers.lock().unwrap_or_else(PoisonError::into_inner).drain() {
            h.abort();
        }
        self.dub_tracks.lock().await.clear();
        if let Err(e) = self.room.close().await {
            warn!(error = %e, "closing LiveKit room failed");
        }
    }
}
