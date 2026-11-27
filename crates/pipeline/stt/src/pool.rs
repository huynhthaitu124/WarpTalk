//! Warm pool of pre-connected Realtime sockets.
//!
//! The TLS + WebSocket handshake costs 1-2 s (spec 08 section 2.2); a speaker's first word must
//! not pay it. The pool keeps `pool_size` idle sockets; [`WarmPool::claim`] hands out the
//! youngest live one instantly and refills in the background. Sockets carry no per-speaker
//! state: the speaker's `session.update` is sent fire-and-forget at claim time, in front of the
//! audio, so it costs no round trip.
//!
//! Hygiene (the upstream lesson: a pool "with no clock" handed out connections the server had
//! already closed, costing the first speaker their first sentence): a socket is discarded at
//! claim when it is closed by the peer (non-blocking poll), older than `max_socket_age_s`, or
//! has less than `min_socket_remaining_s` of its 60-minute life left. `spawn_maintenance` does
//! the same sweep periodically.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::{FutureExt, StreamExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use url::Url;
use warptalk_pipeline_runtime::SttError;

use crate::config::OpenAiSttConfig;

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct WarmSocket {
    pub ws: WsStream,
    pub opened_at: Instant,
}

/// Opens one transcription socket (`?intent=transcription`, bearer auth).
/// Period of the background sweep started by [`WarmPool::warm_up`].
const MAINTENANCE_EVERY: Duration = Duration::from_secs(15);

pub async fn connect(cfg: &OpenAiSttConfig) -> Result<WsStream, SttError> {
    if cfg.api_key.is_empty() {
        return Err(SttError::Connect("api key not configured".into()));
    }
    let mut url =
        Url::parse(&cfg.base_url).map_err(|e| SttError::Connect(format!("bad base_url: {e}")))?;
    if !url.query_pairs().any(|(k, _)| k == "intent") {
        url.query_pairs_mut().append_pair("intent", "transcription");
    }
    let mut req = url
        .as_str()
        .into_client_request()
        .map_err(|e| SttError::Connect(e.to_string()))?;
    let mut auth = HeaderValue::from_str(&format!("Bearer {}", cfg.api_key))
        .map_err(|_| SttError::Connect("api key is not a valid header value".into()))?;
    auth.set_sensitive(true);
    req.headers_mut().insert(AUTHORIZATION, auth);
    match tokio::time::timeout(cfg.connect_timeout(), tokio_tungstenite::connect_async(req)).await {
        Ok(Ok((ws, _resp))) => Ok(ws),
        Ok(Err(e)) => Err(SttError::Connect(e.to_string())),
        Err(_) => Err(SttError::Timeout("connect".into())),
    }
}

/// Non-blocking liveness probe. Swallows buffered informational frames (`session.created`).
fn still_open(ws: &mut WsStream) -> bool {
    loop {
        match ws.next().now_or_never() {
            None => return true,
            Some(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => return false,
            Some(Some(Ok(_))) => {}
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub idle: usize,
    pub warm_claims: u64,
    pub cold_connects: u64,
    pub discarded: u64,
    pub connects_opened: u64,
}

pub struct WarmPool {
    cfg: Arc<OpenAiSttConfig>,
    idle: Mutex<VecDeque<WarmSocket>>,
    inflight: AtomicUsize,
    warm_claims: AtomicU64,
    cold_connects: AtomicU64,
    discarded: AtomicU64,
    opened: AtomicU64,
    /// Set once the periodic sweep runs (started by the first `warm_up`).
    maintained: AtomicBool,
}

impl WarmPool {
    pub fn new(cfg: Arc<OpenAiSttConfig>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            idle: Mutex::new(VecDeque::new()),
            inflight: AtomicUsize::new(0),
            warm_claims: AtomicU64::new(0),
            cold_connects: AtomicU64::new(0),
            discarded: AtomicU64::new(0),
            opened: AtomicU64::new(0),
            maintained: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<WarmSocket>> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            idle: self.lock().len(),
            warm_claims: self.warm_claims.load(Ordering::Relaxed),
            cold_connects: self.cold_connects.load(Ordering::Relaxed),
            discarded: self.discarded.load(Ordering::Relaxed),
            connects_opened: self.opened.load(Ordering::Relaxed),
        }
    }

    /// Opens `pool_size` sockets concurrently and waits for them. Returns how many succeeded.
    pub async fn warm_up(self: &Arc<Self>) -> usize {
        let want = self.cfg.pool_size;
        let results = futures::future::join_all((0..want).map(|_| self.open_one())).await;
        // Without a clock the idle sockets die quietly (peer close, age, network blip) and the
        // first speaker after a quiet spell pays a cold connect: sweep and top up periodically.
        if want > 0 && !self.maintained.swap(true, Ordering::Relaxed) {
            self.spawn_maintenance(MAINTENANCE_EVERY);
        }
        results.into_iter().filter(|ok| *ok).count()
    }

    async fn open_one(&self) -> bool {
        match connect(&self.cfg).await {
            Ok(ws) => {
                self.opened.fetch_add(1, Ordering::Relaxed);
                self.lock().push_back(WarmSocket {
                    ws,
                    opened_at: Instant::now(),
                });
                true
            }
            Err(e) => {
                warn!(error = %e, "stt warm socket failed");
                false
            }
        }
    }

    /// Background top-up to `pool_size` (counting connects already in flight).
    pub fn refill(self: &Arc<Self>) {
        let have = self.lock().len() + self.inflight.load(Ordering::Relaxed);
        for _ in have..self.cfg.pool_size {
            self.inflight.fetch_add(1, Ordering::Relaxed);
            let pool = self.clone();
            tokio::spawn(async move {
                pool.open_one().await;
                pool.inflight.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }

    fn usable(&self, s: &mut WarmSocket) -> bool {
        let age = s.opened_at.elapsed();
        let max = Duration::from_secs(self.cfg.max_socket_age_s);
        let min_left = Duration::from_secs(self.cfg.min_socket_remaining_s);
        age < max && max.saturating_sub(age) >= min_left && still_open(&mut s.ws)
    }

    /// Youngest usable warm socket, else a cold connect. Always triggers a background refill.
    pub async fn claim(self: &Arc<Self>) -> Result<WarmSocket, SttError> {
        let mut found = None;
        loop {
            // Youngest first: the refill pushes on the right; it has the most life left.
            let Some(mut s) = self.lock().pop_back() else {
                break;
            };
            if self.usable(&mut s) {
                found = Some(s);
                break;
            }
            self.discarded.fetch_add(1, Ordering::Relaxed);
            debug!("discarded unusable warm socket");
        }
        self.refill();
        if let Some(s) = found {
            self.warm_claims.fetch_add(1, Ordering::Relaxed);
            return Ok(s);
        }
        self.cold_connects.fetch_add(1, Ordering::Relaxed);
        let ws = connect(&self.cfg).await?;
        self.opened.fetch_add(1, Ordering::Relaxed);
        Ok(WarmSocket {
            ws,
            opened_at: Instant::now(),
        })
    }

    /// Drops dead/aged sockets and tops the pool up.
    pub fn sweep(self: &Arc<Self>) {
        let mut idle = self.lock();
        let before = idle.len();
        let mut kept = VecDeque::with_capacity(before);
        while let Some(mut s) = idle.pop_front() {
            if self.usable(&mut s) {
                kept.push_back(s);
            } else {
                self.discarded.fetch_add(1, Ordering::Relaxed);
            }
        }
        *idle = kept;
        drop(idle);
        self.refill();
    }

    /// Periodic [`sweep`](Self::sweep).
    pub fn spawn_maintenance(self: &Arc<Self>, every: Duration) -> JoinHandle<()> {
        let pool = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                pool.sweep();
            }
        })
    }
}
