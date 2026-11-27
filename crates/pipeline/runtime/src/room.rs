//! Room actor: owns the speakers of one room and the hot-reloadable config.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::config::PipelineConfig;
use crate::error::PipelineError;
use crate::messages::{ParticipantId, RoomId, SttSegment};
use crate::speaker::{SpeakerHandle, SpeakerPipeline, SpeakerSpec, VadFactory};
use crate::stt::SpeechToText;

pub struct RoomDeps {
    pub room: RoomId,
    pub stt: Arc<dyn SpeechToText>,
    pub vad_factory: VadFactory,
    /// Optional process-wide shutdown signal (`true` = shut down). The room also has its own,
    /// see [`RoomPipeline::shutdown`].
    pub external_shutdown: Option<watch::Receiver<bool>>,
}

enum Cmd {
    Add {
        id: ParticipantId,
        lang: Option<String>,
        languages: Vec<String>,
        reply: oneshot::Sender<Result<SpeakerHandle, PipelineError>>,
    },
    Remove {
        id: ParticipantId,
        reply: oneshot::Sender<Result<(), PipelineError>>,
    },
    Update {
        cfg: Box<PipelineConfig>,
        reply: oneshot::Sender<Result<(), PipelineError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Cloneable handle to the room actor.
#[derive(Clone)]
pub struct RoomPipeline {
    cmd: mpsc::Sender<Cmd>,
    shutdown: watch::Sender<bool>,
    done: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl RoomPipeline {
    /// Starts the actor. Segments of all speakers arrive on the returned receiver (bounded by
    /// `segment_channel_capacity`); per-speaker order is preserved via `SttSegment::seq`.
    pub fn spawn(
        deps: RoomDeps,
        cfg: PipelineConfig,
    ) -> Result<(Self, mpsc::Receiver<SttSegment>), PipelineError> {
        cfg.validate()?;
        let (out_tx, out_rx) = mpsc::channel(cfg.segment_channel_capacity);
        let (cfg_tx, cfg_rx) = watch::channel(Arc::new(cfg));
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let actor = Actor {
            deps,
            cfg_tx,
            cfg_rx,
            out: out_tx,
            speakers: HashMap::new(),
            shutdown_rx,
        };
        let join = tokio::spawn(actor.run(cmd_rx));
        Ok((
            Self {
                cmd: cmd_tx,
                shutdown: shutdown_tx,
                done: Arc::new(tokio::sync::Mutex::new(Some(join))),
            },
            out_rx,
        ))
    }

    pub async fn add_speaker(
        &self,
        id: impl Into<ParticipantId>,
        lang: Option<String>,
    ) -> Result<SpeakerHandle, PipelineError> {
        self.add_speaker_with(id, lang, Vec::new()).await
    }

    /// [`Self::add_speaker`] with the room's declared languages (STT code-switching hints).
    pub async fn add_speaker_with(
        &self,
        id: impl Into<ParticipantId>,
        lang: Option<String>,
        languages: Vec<String>,
    ) -> Result<SpeakerHandle, PipelineError> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Add {
            id: id.into(),
            lang,
            languages,
            reply,
        })
        .await?;
        rx.await.map_err(|_| PipelineError::RoomClosed)?
    }

    /// Removes the speaker; resolves after its queue is drained and its transcripts forwarded.
    pub async fn remove_speaker(&self, id: impl Into<ParticipantId>) -> Result<(), PipelineError> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Remove {
            id: id.into(),
            reply,
        })
        .await?;
        rx.await.map_err(|_| PipelineError::RoomClosed)?
    }

    /// Hot reload. Validated first; speakers apply it at their next utterance.
    pub async fn update_config(&self, cfg: PipelineConfig) -> Result<(), PipelineError> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Update {
            cfg: Box::new(cfg),
            reply,
        })
        .await?;
        rx.await.map_err(|_| PipelineError::RoomClosed)?
    }

    /// Graceful shutdown: no new speakers, every speaker drains (queued frames processed, open
    /// utterance committed, transcripts forwarded), then the output channel closes.
    pub async fn shutdown(&self) {
        let _ = self.shutdown.send(true);
        let (reply, rx) = oneshot::channel();
        if self.cmd.send(Cmd::Shutdown { reply }).await.is_ok() {
            let _ = rx.await;
        }
        if let Some(j) = self.done.lock().await.take() {
            let _ = j.await;
        }
    }

    async fn send(&self, cmd: Cmd) -> Result<(), PipelineError> {
        self.cmd
            .send(cmd)
            .await
            .map_err(|_| PipelineError::RoomClosed)
    }
}

struct Actor {
    deps: RoomDeps,
    cfg_tx: watch::Sender<Arc<PipelineConfig>>,
    cfg_rx: watch::Receiver<Arc<PipelineConfig>>,
    out: mpsc::Sender<SttSegment>,
    speakers: HashMap<ParticipantId, (SpeakerHandle, SpeakerPipeline)>,
    shutdown_rx: watch::Receiver<bool>,
}

impl Actor {
    async fn run(mut self, mut cmd_rx: mpsc::Receiver<Cmd>) {
        let mut external = self.deps.external_shutdown.take();
        let mut shutdown_reply = None;
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown_rx.changed() => break,
                _ = wait_external(&mut external) => break,
                cmd = cmd_rx.recv() => match cmd {
                    None => break,
                    Some(Cmd::Shutdown { reply }) => { shutdown_reply = Some(reply); break; }
                    Some(cmd) => self.handle(cmd),
                },
            }
        }
        // Stop accepting commands, drain every speaker concurrently.
        cmd_rx.close();
        let speakers = std::mem::take(&mut self.speakers);
        info!(room = %self.deps.room, speakers = speakers.len(), "room pipeline shutting down");
        for (_, sp) in speakers.values() {
            sp.close_input();
        }
        for (_, (_, sp)) in speakers {
            let _ = sp.into_join().await;
        }
        drop(self.out);
        if let Some(r) = shutdown_reply {
            let _ = r.send(());
        }
        // Commands that raced the shutdown get RoomClosed when their reply sender drops.
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Add {
                id,
                lang,
                languages,
                reply,
            } => {
                let res = if self.speakers.contains_key(&id) {
                    Err(PipelineError::SpeakerExists(id))
                } else {
                    let sp = SpeakerPipeline::spawn(
                        SpeakerSpec {
                            room: self.deps.room.clone(),
                            id: id.clone(),
                            lang,
                            languages,
                        },
                        self.deps.stt.clone(),
                        &self.deps.vad_factory,
                        self.cfg_rx.clone(),
                        self.out.clone(),
                    );
                    let h = sp.handle();
                    self.speakers.insert(id, (h.clone(), sp));
                    Ok(h)
                };
                let _ = reply.send(res);
            }
            Cmd::Remove { id, reply } => match self.speakers.remove(&id) {
                None => {
                    let _ = reply.send(Err(PipelineError::SpeakerNotFound(id)));
                }
                Some((_, sp)) => {
                    // Drain off the actor loop so other commands are not blocked.
                    sp.close_input();
                    let join = sp.into_join();
                    tokio::spawn(async move {
                        let _ = join.await;
                        let _ = reply.send(Ok(()));
                    });
                }
            },
            Cmd::Update { cfg, reply } => {
                let res = cfg.validate().map(|()| {
                    debug!(room = %self.deps.room, "pipeline config updated");
                    let _ = self.cfg_tx.send(Arc::new(*cfg));
                });
                let _ = reply.send(res);
            }
            Cmd::Shutdown { .. } => {}
        }
    }
}

async fn wait_external(rx: &mut Option<watch::Receiver<bool>>) {
    match rx {
        Some(r) => {
            if *r.borrow() {
                return;
            }
            while r.changed().await.is_ok() {
                if *r.borrow() {
                    return;
                }
            }
            std::future::pending::<()>().await
        }
        None => std::future::pending::<()>().await,
    }
}
