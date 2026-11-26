//! [`CopilotEngine`]: two channels (`me`, `others`), shared sink and STT backend.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use warptalk_ingress::EndpointerConfig;
use warptalk_pipeline_runtime::SpeechToText;

use crate::channel::{Counters, EventTx, Worker, WorkerCtx};
use crate::config::{ChannelConfig, CopilotConfig, SttRouting};
use crate::echo::LevelHistory;
use crate::error::CopilotError;
use crate::queue::{Chunk, ChunkQueue};
use crate::sink::SegmentSink;
use crate::stt::{NoticeBoard, TimingBoard};
use crate::types::{Channel, ChannelStats, CopilotEvent, EngineStats, PushStatus, StatusKind};

pub struct CopilotDeps {
    pub stt: Arc<dyn SpeechToText>,
    pub sink: Arc<dyn SegmentSink>,
    pub session_id: String,
}

/// Optional extras beyond [`CopilotDeps`]: the server-socket STT used for fallback / `server`
/// routing and the boards it shares with the engine (see `stt::ServerAudioStt`).
#[derive(Default)]
pub struct EngineExtras {
    pub server_stt: Option<Arc<dyn SpeechToText>>,
    pub timing: Option<Arc<TimingBoard>>,
    pub notices: Option<Arc<NoticeBoard>>,
}

struct Chan {
    queue: Arc<ChunkQueue>,
    hist: Arc<Mutex<LevelHistory>>,
    cfg_tx: watch::Sender<ChannelConfig>,
    counters: Arc<Counters>,
    pushed: AtomicU64,
    enabled: bool,
}

pub struct CopilotEngine {
    chans: [Chan; 2],
    sink: Arc<dyn SegmentSink>,
    events: EventTx,
    events_dropped: Arc<AtomicU64>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    ticker: Mutex<Option<JoinHandle<()>>>,
    stopped: AtomicBool,
}

impl CopilotEngine {
    /// Validates the config, spawns the channel workers (needs a tokio runtime) and returns
    /// the engine plus the single event receiver.
    pub fn new(
        cfg: CopilotConfig,
        deps: CopilotDeps,
    ) -> Result<(Self, mpsc::Receiver<CopilotEvent>), CopilotError> {
        Self::with_extras(cfg, deps, EngineExtras::default())
    }

    /// Like [`Self::new`] with a server-socket STT for `auto` fallback or `server` routing.
    pub fn with_extras(
        cfg: CopilotConfig,
        deps: CopilotDeps,
        extras: EngineExtras,
    ) -> Result<(Self, mpsc::Receiver<CopilotEvent>), CopilotError> {
        cfg.validate()?;
        if cfg.stt.mode == SttRouting::Server && extras.server_stt.is_none() {
            return Err(CopilotError::Config(
                "stt.mode = server needs a server STT backend".into(),
            ));
        }
        let timing = extras.timing.unwrap_or_default();
        let fallback_cfg = Arc::new(cfg.stt.fallback.clone());
        let language_cfg = Arc::new(cfg.language.clone());
        let (ev_tx, ev_rx) = mpsc::channel(cfg.engine.event_capacity);
        let events_dropped = Arc::new(AtomicU64::new(0));
        let events = EventTx::new(ev_tx, events_dropped.clone());
        let engine_cfg = Arc::new(cfg.engine.clone());
        let echo_cfg = Arc::new(cfg.echo.clone());
        let hists = [
            Arc::new(Mutex::new(LevelHistory::default())),
            Arc::new(Mutex::new(LevelHistory::default())),
        ];
        let mut tasks = Vec::new();
        let mut chans = Vec::new();
        for ch in Channel::ALL {
            let ccfg = cfg.channel(ch).clone();
            let (cfg_tx, cfg_rx) = watch::channel(ccfg.clone());
            let queue = Arc::new(ChunkQueue::new(ccfg.queue_capacity_chunks));
            let counters = Arc::new(Counters::default());
            if ccfg.enabled {
                let worker = Worker::new(WorkerCtx {
                    channel: ch,
                    session_id: deps.session_id.clone(),
                    cfg_rx,
                    engine: engine_cfg.clone(),
                    echo: echo_cfg.clone(),
                    queue: queue.clone(),
                    stt: deps.stt.clone(),
                    server_stt: extras.server_stt.clone(),
                    routing: cfg.stt.mode,
                    fallback: fallback_cfg.clone(),
                    language: language_cfg.clone(),
                    timing: timing.clone(),
                    sink: deps.sink.clone(),
                    events: events.clone(),
                    my_hist: hists[ch.index()].clone(),
                    other_hist: hists[ch.other().index()].clone(),
                    counters: counters.clone(),
                    open_timeout: Duration::from_millis(cfg.stt.open_timeout_ms),
                });
                tasks.push(tokio::spawn(worker.run()));
            }
            chans.push(Chan {
                queue,
                hist: hists[ch.index()].clone(),
                cfg_tx,
                counters,
                pushed: AtomicU64::new(0),
                enabled: ccfg.enabled,
            });
        }
        let [me, others] = <[Chan; 2]>::try_from(chans)
            .map_err(|_| CopilotError::Config("internal: channel setup".into()))?;
        let engine = Self {
            chans: [me, others],
            sink: deps.sink.clone(),
            events: events.clone(),
            events_dropped,
            tasks: Mutex::new(tasks),
            ticker: Mutex::new(None),
            stopped: AtomicBool::new(false),
        };
        engine.start_ticker(
            Duration::from_millis(cfg.engine.status_interval_ms),
            extras.notices,
        );
        engine.events.emit(CopilotEvent::Status {
            channel: None,
            kind: StatusKind::Started,
            detail: None,
        });
        Ok((engine, ev_rx))
    }

    fn start_ticker(&self, period: Duration, notices: Option<Arc<NoticeBoard>>) {
        let sink = self.sink.clone();
        let events = self.events.clone();
        let queues = [self.chans[0].queue.clone(), self.chans[1].queue.clone()];
        let h = tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            let mut offline = false;
            let mut unauth = false;
            let mut last_dropped = [0u64; 2];
            loop {
                tick.tick().await;
                if let Some(n) = &notices {
                    for (code, message) in n.drain() {
                        events.emit(CopilotEvent::Error {
                            channel: None,
                            code,
                            message,
                        });
                    }
                }
                let un = sink.unauthorized();
                if un && !unauth {
                    events.emit(CopilotEvent::Error {
                        channel: None,
                        code: "auth_required".into(),
                        message: "segment upload rejected with http 401".into(),
                    });
                }
                unauth = un;
                let off = sink.stats().offline;
                if off != offline {
                    offline = off;
                    events.emit(CopilotEvent::Status {
                        channel: None,
                        kind: if off {
                            StatusKind::SinkOffline
                        } else {
                            StatusKind::SinkOnline
                        },
                        detail: None,
                    });
                }
                for ch in Channel::ALL {
                    let d = queues[ch.index()].dropped();
                    if d != last_dropped[ch.index()] {
                        events.emit(CopilotEvent::Status {
                            channel: Some(ch),
                            kind: StatusKind::AudioDropped,
                            detail: Some((d - last_dropped[ch.index()]).to_string()),
                        });
                        last_dropped[ch.index()] = d;
                    }
                }
            }
        });
        *self.ticker.lock().unwrap_or_else(PoisonError::into_inner) = Some(h);
    }

    /// Push mono f32 samples at any sample rate. Synchronous, never blocks, never fails:
    /// when the channel's queue is full the oldest chunk is evicted and counted.
    pub fn push_audio(&self, channel: Channel, samples: &[f32], sample_rate: u32) -> PushStatus {
        let c = &self.chans[channel.index()];
        if samples.is_empty() || sample_rate == 0 || !c.enabled {
            return PushStatus::Ignored;
        }
        if self.stopped.load(Ordering::Relaxed) {
            return PushStatus::Closed;
        }
        let start_ms = {
            let mut h = c.hist.lock().unwrap_or_else(PoisonError::into_inner);
            let s = h.elapsed_ms();
            h.feed(samples, sample_rate);
            s
        };
        c.pushed.fetch_add(1, Ordering::Relaxed);
        c.queue.push(Chunk {
            samples: samples.to_vec(),
            sample_rate,
            start_ms,
        })
    }

    /// Hot-reload the endpointer of one channel (applied when that channel is idle).
    pub fn update_endpointer(
        &self,
        channel: Channel,
        endpointer: EndpointerConfig,
    ) -> Result<(), CopilotError> {
        let tx = &self.chans[channel.index()].cfg_tx;
        let mut next = tx.borrow().clone();
        next.endpointer = endpointer;
        self.update_channel_config(channel, next)
    }

    pub fn update_channel_config(
        &self,
        channel: Channel,
        cfg: ChannelConfig,
    ) -> Result<(), CopilotError> {
        cfg.validate(channel.as_str())?;
        self.chans[channel.index()].cfg_tx.send_replace(cfg);
        Ok(())
    }

    pub fn stats(&self) -> EngineStats {
        let one = |ch: Channel| {
            let c = &self.chans[ch.index()];
            ChannelStats {
                chunks_pushed: c.pushed.load(Ordering::Relaxed),
                chunks_dropped: c.queue.dropped(),
                utterances: c.counters.utterances.load(Ordering::Relaxed),
                segments: c.counters.segments.load(Ordering::Relaxed),
                echo_marked: c.counters.echo_marked.load(Ordering::Relaxed),
                stt_errors: c.counters.stt_errors.load(Ordering::Relaxed),
                stt_sessions_opened: c.counters.sessions_opened.load(Ordering::Relaxed),
                stt_route: c.counters.route(),
                stt_fallbacks: c.counters.fallbacks.load(Ordering::Relaxed),
            }
        };
        EngineStats {
            me: one(Channel::Me),
            others: one(Channel::Others),
            sink: self.sink.stats(),
            events_dropped: self.events_dropped.load(Ordering::Relaxed),
        }
    }

    /// Finishes open utterances, waits for in-flight transcripts, flushes and closes the sink.
    /// Idempotent.
    pub async fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        for c in &self.chans {
            c.queue.close();
        }
        let tasks: Vec<_> =
            std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        for t in tasks {
            let _ = t.await;
        }
        if let Some(t) = self
            .ticker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            t.abort();
        }
        self.sink.close().await;
        self.events.emit(CopilotEvent::Status {
            channel: None,
            kind: StatusKind::Stopped,
            detail: None,
        });
    }
}
