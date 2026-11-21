//! Warm pool of pre-dialled Cartesia sockets.
//!
//! The dial (TCP + TLS + WebSocket) measured p50 427.6 ms upstream against 0.1 ms to open a
//! context, so roughly three quarters of the first-sentence wait is a handshake that can happen
//! at any earlier moment (spec 08 section 4 item 7). A socket carries no voice, language or
//! model (those belong to a context), so the pool can be filled at startup.
//!
//! Hygiene follows the upstream lesson (a dead socket handed to a sentence is worse than a fresh
//! dial): age is checked at claim time (no timer), plus a non-blocking liveness probe that
//! swallows buffered frames and rejects a socket the peer already closed. Claiming always
//! schedules a background refill; a failed refill stops instead of spinning (the next claim
//! tries again), so an outage costs cold dials, never a reconnect loop.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::{FutureExt, StreamExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use crate::config::CartesiaConfig;
use crate::error::CartesiaError;

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct WarmSocket {
    pub ws: WsStream,
    pub opened_at: Instant,
}

/// Opens one TTS socket. Auth by header (never in the URL, so it cannot leak into logs).
pub async fn connect(cfg: &CartesiaConfig) -> Result<WsStream, CartesiaError> {
    if cfg.api_key.is_empty() {
        return Err(CartesiaError::NotConfigured("api key"));
    }
    let mut req = cfg
        .ws_url
        .as_str()
        .into_client_request()
        .map_err(|e| CartesiaError::Connect(e.to_string()))?;
    let mut key = HeaderValue::from_str(&cfg.api_key)
        .map_err(|_| CartesiaError::NotConfigured("api key is not a valid header value"))?;
    key.set_sensitive(true);
    let version = HeaderValue::from_str(&cfg.api_version)
        .map_err(|_| CartesiaError::NotConfigured("api version is not a valid header value"))?;
    req.headers_mut().insert("x-api-key", key);
    req.headers_mut().insert("cartesia-version", version);
    match tokio::time::timeout(cfg.connect_timeout(), tokio_tungstenite::connect_async(req)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(CartesiaError::Connect(e.to_string())),
        Err(_) => Err(CartesiaError::Timeout("connect")),
    }
}

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
    cfg: Arc<CartesiaConfig>,
    idle: Mutex<VecDeque<WarmSocket>>,
    inflight: AtomicUsize,
    warm_claims: AtomicU64,
    cold_connects: AtomicU64,
    discarded: AtomicU64,
    opened: AtomicU64,
}

impl WarmPool {
    pub fn new(cfg: Arc<CartesiaConfig>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            idle: Mutex::new(VecDeque::new()),
            inflight: AtomicUsize::new(0),
            warm_claims: AtomicU64::new(0),
            cold_connects: AtomicU64::new(0),
            discarded: AtomicU64::new(0),
            opened: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<WarmSocket>> {
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

    /// Dials `pool_size` sockets concurrently and waits. Returns how many succeeded.
    pub async fn warm_up(self: &Arc<Self>) -> usize {
        let want = self.cfg.pool_size;
        let results = futures::future::join_all((0..want).map(|_| self.open_one())).await;
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
                warn!(error = %e, "cartesia warm socket failed");
                false
            }
        }
    }

    /// Background top-up to `pool_size`, counting dials already in flight.
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
        s.opened_at.elapsed() <= self.cfg.max_idle() && still_open(&mut s.ws)
    }

    /// Youngest usable warm socket, else a cold dial. Always triggers a background refill (an
    /// empty pool is the case that most needs one, and a claim is the only sign of traffic).
    pub async fn claim(self: &Arc<Self>) -> Result<WarmSocket, CartesiaError> {
        let mut found = None;
        loop {
            let Some(mut s) = self.lock().pop_back() else {
                break;
            };
            if self.usable(&mut s) {
                found = Some(s);
                break;
            }
            self.discarded.fetch_add(1, Ordering::Relaxed);
            debug!("discarded unusable cartesia socket");
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

    /// Drops dead or aged sockets and tops the pool up.
    pub fn sweep(self: &Arc<Self>) {
        let mut idle = self.lock();
        let mut kept = VecDeque::with_capacity(idle.len());
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
