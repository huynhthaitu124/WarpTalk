//! [`CopilotHandle`]: the facade the macOS app embeds (see the crate docs for the UniFFI
//! surface). Owns its own tokio runtime so foreign callers need no async; every method is
//! synchronous. Do not drop it from inside another tokio runtime.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::{CopilotConfig, SttBackend};
use crate::engine::{CopilotDeps, CopilotEngine};
use crate::error::CopilotError;
use crate::sink::HttpSegmentSink;
use crate::stt::{EphemeralRealtimeStt, FakeStt};
use crate::token::{HttpSttTokenProvider, TokenManager};
use crate::types::{Channel, CopilotEvent, EngineStats, PushStatus};

/// Flat record for `start`.
#[derive(Debug, Clone)]
pub struct StartParams {
    /// Server base URL, e.g. `https://api.example.com` (no trailing path).
    pub base_url: String,
    pub session_id: String,
    /// Bearer for the copilot API (segments + stt-token).
    pub bearer: String,
}

/// Pull-style event stream (UniFFI object). `next` blocks up to `timeout_ms`.
pub struct EventStream {
    rt: Arc<Runtime>,
    rx: Mutex<Option<mpsc::Receiver<CopilotEvent>>>,
}

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
    refresher: Option<JoinHandle<()>>,
}

pub struct CopilotHandle {
    rt: Arc<Runtime>,
    cfg: CopilotConfig,
    events: Arc<EventStream>,
    running: Mutex<Option<Running>>,
}

impl CopilotHandle {
    /// `config_json`: partial JSON of [`CopilotConfig`] (`"{}"` = all defaults).
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
        let (stt, tokens, refresher): (Arc<dyn warptalk_pipeline_runtime::SpeechToText>, _, _) =
            match self.cfg.stt.backend {
                SttBackend::Fake => (
                    Arc::new(FakeStt::new(Duration::from_millis(
                        self.cfg.stt.fake_latency_ms,
                    ))),
                    None,
                    None,
                ),
                SttBackend::Realtime => {
                    let prov = Arc::new(HttpSttTokenProvider::new(
                        &params.base_url,
                        &params.session_id,
                        params.bearer.clone(),
                        Duration::from_millis(self.cfg.stt.open_timeout_ms),
                    )?);
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
        let (engine, rx) = CopilotEngine::new(
            self.cfg.clone(),
            CopilotDeps {
                stt,
                sink: sink.clone(),
                session_id: params.session_id,
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
                t.set_bearer(bearer);
            }
        }
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
}
