//! [`CopilotHandle`]: the facade the macOS app embeds (see the crate docs for the UniFFI
//! surface). Owns its own tokio runtime so foreign callers need no async; every method is
//! synchronous. Do not drop it from inside another tokio runtime.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::{CopilotConfig, SttBackend, SttRouting};
use crate::engine::{CopilotDeps, CopilotEngine, EngineExtras};
use crate::error::CopilotError;
use crate::sink::HttpSegmentSink;
use crate::stt::{EphemeralRealtimeStt, FakeStt, NoticeBoard, ServerAudioStt, TimingBoard};
use crate::token::{HttpSttTokenProvider, TokenManager};
use crate::types::{Channel, CopilotEvent, EngineStats, PushStatus};

/// Flat record for `start`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StartParams {
    /// Server base URL, e.g. `https://api.example.com` (no trailing path).
    pub base_url: String,
    pub session_id: String,
    /// Bearer for the copilot API (segments + stt-token).
    pub bearer: String,
}

/// Pull-style event stream (UniFFI object). `next` blocks up to `timeout_ms`.
#[cfg_attr(feature = "ffi", derive(uniffi::Object))]
pub struct EventStream {
    rt: Arc<Runtime>,
    rx: Mutex<Option<mpsc::Receiver<CopilotEvent>>>,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl EventStream {
    pub fn next(&self, timeout_ms: u64) -> Option<CopilotEvent> {
        let mut g = self.rx.lock().unwrap_or_else(PoisonError::into_inner);
        let rx = g.as_mut()?;
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(timeout_ms), rx.recv())
                .await
                .ok()
                .flatten()
        })
    }

    /// Non-blocking drain of up to `max` events.
    pub fn drain(&self, max: u32) -> Vec<CopilotEvent> {
        let mut g = self.rx.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(rx) = g.as_mut() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while out.len() < max as usize {
            match rx.try_recv() {
                Ok(e) => out.push(e),
                Err(_) => break,
            }
        }
        out
    }
}

struct Running {
    engine: Arc<CopilotEngine>,
    sink: Arc<HttpSegmentSink>,
    tokens: Option<Arc<HttpSttTokenProvider>>,
    server: Option<Arc<ServerAudioStt>>,
    refresher: Option<JoinHandle<()>>,
}

#[cfg_attr(feature = "ffi", derive(uniffi::Object))]
pub struct CopilotHandle {
    rt: Arc<Runtime>,
    listener: Arc<Mutex<Option<Arc<dyn CopilotEventListener>>>>,
    pump_started: std::sync::atomic::AtomicBool,
    cfg: CopilotConfig,
    events: Arc<EventStream>,
    running: Mutex<Option<Running>>,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl CopilotHandle {
    /// `config_json`: partial JSON of [`CopilotConfig`] (`"{}"` = all defaults).
    #[cfg_attr(feature = "ffi", uniffi::constructor)]
    pub fn new(config_json: String) -> Result<Arc<Self>, CopilotError> {
        let cfg = CopilotConfig::from_json(&config_json)?;
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("copilot-core")
                .build()
                .map_err(|e| CopilotError::Io(e.to_string()))?,
        );
        Ok(Arc::new(Self {
            events: Arc::new(EventStream {
                rt: rt.clone(),
                rx: Mutex::new(None),
            }),
            rt,
            cfg,
            listener: Arc::new(Mutex::new(None)),
            pump_started: std::sync::atomic::AtomicBool::new(false),
            running: Mutex::new(None),
        }))
    }

    pub fn start(&self, params: StartParams) -> Result<(), CopilotError> {
        let mut g = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if g.is_some() {
            return Err(CopilotError::AlreadyStarted);
        }
        let _enter = self.rt.enter();
        let sink = HttpSegmentSink::spawn(
            self.cfg.sink.clone(),
            &params.base_url,
            &params.session_id,
            params.bearer.clone(),
        )?;
        let routing = self.cfg.stt.mode;
        let timing = Arc::new(TimingBoard::default());
        let notices = Arc::new(NoticeBoard::default());
        let server = (routing != SttRouting::Direct)
            .then(|| {
                ServerAudioStt::new(
                    self.cfg.stt.server.clone(),
                    &self.cfg.language,
                    &params.base_url,
                    &params.session_id,
                    params.bearer.clone(),
                    timing.clone(),
                    notices.clone(),
                )
                .map(Arc::new)
            })
            .transpose()?;
        let (stt, tokens, refresher): (Arc<dyn warptalk_pipeline_runtime::SpeechToText>, _, _) =
            match (routing, self.cfg.stt.backend) {
                // Server routing never touches the direct path: the server socket stands in.
                (SttRouting::Server, _) => match &server {
                    Some(s) => (s.clone(), None, None),
                    None => return Err(CopilotError::Config("server STT unavailable".into())),
                },
                (_, SttBackend::Fake) => (
                    Arc::new(FakeStt::new(Duration::from_millis(
                        self.cfg.stt.fake_latency_ms,
                    ))),
                    None,
                    None,
                ),
                (_, SttBackend::Realtime) => {
                    let prov = Arc::new(HttpSttTokenProvider::new(
                        &params.base_url,
                        &params.session_id,
                        params.bearer.clone(),
                        Duration::from_millis(self.cfg.stt.open_timeout_ms),
                    )?);
                    prov.set_prefs(&self.cfg.language);
                    let mgr = TokenManager::new(
                        prov.clone(),
                        Duration::from_secs(self.cfg.stt.token_refresh_margin_s),
                        self.cfg.stt.reuse_token,
                    );
                    let refresher = self
                        .cfg
                        .stt
                        .reuse_token
                        .then(|| mgr.spawn_refresher(Duration::from_secs(5)));
                    (
                        Arc::new(EphemeralRealtimeStt::new(self.cfg.stt.clone(), mgr)),
                        Some(prov),
                        refresher,
                    )
                }
            };
        let (engine, rx) = CopilotEngine::with_extras(
            self.cfg.clone(),
            CopilotDeps {
                stt,
                sink: sink.clone(),
                session_id: params.session_id,
            },
            EngineExtras {
                server_stt: server
                    .clone()
                    .map(|s| s as Arc<dyn warptalk_pipeline_runtime::SpeechToText>),
                timing: Some(timing),
                notices: Some(notices),
            },
        )?;
        *self
            .events
            .rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(rx);
        *g = Some(Running {
            engine: Arc::new(engine),
            sink,
            tokens,
            server,
            refresher,
        });
        Ok(())
    }

    /// Never blocks. Before `start` / after `stop` the audio is ignored (`Closed`).
    pub fn push_audio(&self, channel: Channel, samples: Vec<f32>, sample_rate: u32) -> PushStatus {
        let engine = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|r| r.engine.clone());
        match engine {
            Some(e) => e.push_audio(channel, &samples, sample_rate),
            None => PushStatus::Closed,
        }
    }

    /// Replace the bearer (after the app refreshed its access token).
    pub fn update_bearer(&self, bearer: String) {
        if let Some(r) = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            r.sink.set_bearer(bearer.clone());
            if let Some(t) = &r.tokens {
                t.set_bearer(bearer.clone());
            }
            if let Some(sv) = &r.server {
                sv.set_bearer(bearer);
            }
        }
    }

    /// The configured STT routing (`auto` | `direct` | `server`). Where each channel runs *now*
    /// is `stats().me.stt_route` / `.others.stt_route`; switches arrive as `SttFallback` /
    /// `SttRecovered` status events.
    pub fn stt_mode(&self) -> SttRouting {
        self.cfg.stt.mode
    }

    /// Hot-reload one channel's endpointer from JSON (partial `EndpointerConfig`).
    pub fn update_endpointer(
        &self,
        channel: Channel,
        endpointer_json: String,
    ) -> Result<(), CopilotError> {
        let ep: warptalk_ingress::EndpointerConfig = serde_json::from_str(&endpointer_json)
            .map_err(|e| CopilotError::Config(e.to_string()))?;
        let engine = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|r| r.engine.clone())
            .ok_or(CopilotError::NotStarted)?;
        engine.update_endpointer(channel, ep)
    }

    pub fn stats(&self) -> Result<EngineStats, CopilotError> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|r| r.engine.stats())
            .ok_or(CopilotError::NotStarted)
    }

    /// Flushes and stops; events already queued stay readable.
    pub fn stop(&self) -> Result<(), CopilotError> {
        let r = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(CopilotError::NotStarted)?;
        if let Some(h) = &r.refresher {
            h.abort();
        }
        self.rt.block_on(r.engine.stop());
        Ok(())
    }

    pub fn events(&self) -> Arc<EventStream> {
        self.events.clone()
    }

    /// Push-style delivery: `listener.on_event` is called for every event, on a dedicated
    /// Rust thread (NOT the main thread: hop to the main actor before touching UI). Replaces
    /// any previous listener. Do not combine with polling `events()` (they share one queue).
    /// The listener must return quickly; it never runs on the audio thread.
    pub fn set_listener(&self, listener: Arc<dyn CopilotEventListener>) {
        *self.listener.lock().unwrap_or_else(PoisonError::into_inner) = Some(listener);
        if self
            .pump_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let events = Arc::downgrade(&self.events);
        let slot = self.listener.clone();
        // Best effort: if the thread cannot be spawned the app can still poll `events()`.
        let _ = std::thread::Builder::new()
            .name("copilot-listener".into())
            .spawn(move || {
                loop {
                    // Hold the stream only for one poll so dropping the handle stops the pump.
                    let Some(stream) = events.upgrade() else {
                        break;
                    };
                    let ev = stream.next(100);
                    drop(stream);
                    match ev {
                        Some(ev) => {
                            let l = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
                            if let Some(l) = l {
                                l.on_event(ev);
                            }
                        }
                        // No receiver yet (before `start`): `next` returns at once.
                        None => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            });
    }
}

/// Foreign callback for [`CopilotHandle::set_listener`].
#[cfg_attr(feature = "ffi", uniffi::export(with_foreign))]
pub trait CopilotEventListener: Send + Sync {
    fn on_event(&self, event: CopilotEvent);
}
