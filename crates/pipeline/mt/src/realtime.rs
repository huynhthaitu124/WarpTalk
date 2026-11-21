//! OpenAI Realtime used as a **text-only translation** engine, over a warm socket pool.
//!
//! Same approach as `warptalk-stt` (pre-connected sockets, claim instantly, background refill,
//! liveness probe and age check at claim) but the sockets are exclusive per request and returned
//! to the pool when healthy (spec 08 section 4 item 8): each request is an isolated, stateless
//! response (`conversation: "none"`), so a socket carries no per-speaker state.
//!
//! Wire shapes follow the upstream working integration (`translator.py::_translate_realtime`).
//! UNVERIFIED against the current docs (host unreachable from the build sandbox):
//! - endpoint `wss://api.openai.com/v1/realtime?model=<model>`, bearer auth;
//! - client event `response.create` with `response: { conversation, input[], instructions,
//!   max_output_tokens, metadata, output_modalities: ["text"], reasoning: { effort } }`;
//! - server events `response.created` (`response.id`, `response.metadata`),
//!   `response.output_text.delta` (`response_id`, `delta`), `response.done`
//!   (`response.id`, `response.status`), `error`.
//!
//! A timed-out or failed call drops its socket (stray late events must never leak into the next
//! request) and triggers a refill.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use url::Url;

use crate::config::RealtimeConfig;
use crate::error::MtError;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Sock {
    ws: WsStream,
    opened_at: Instant,
}

/// Per-call knobs, read from the hot-reloadable config at call time.
#[derive(Debug, Clone)]
pub struct RealtimeCall {
    pub effort: String,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

impl From<&RealtimeConfig> for RealtimeCall {
    fn from(c: &RealtimeConfig) -> Self {
        Self {
            effort: c.reasoning_effort.clone(),
            max_output_tokens: c.max_output_tokens,
            timeout: c.timeout(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeReply {
    pub text: String,
    /// Arrival of the first text delta: the `mt_first_token` stamp.
    pub first_token_at: Instant,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeStats {
    pub idle: usize,
    pub warm_claims: u64,
    pub cold_connects: u64,
    pub discarded: u64,
}

pub struct RealtimeClient {
    cfg: RealtimeConfig,
    idle: Mutex<VecDeque<Sock>>,
    inflight: AtomicUsize,
    warm_claims: AtomicU64,
    cold_connects: AtomicU64,
    discarded: AtomicU64,
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

impl RealtimeClient {
    pub fn new(cfg: RealtimeConfig) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            idle: Mutex::new(VecDeque::new()),
            inflight: AtomicUsize::new(0),
            warm_claims: AtomicU64::new(0),
            cold_connects: AtomicU64::new(0),
            discarded: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Sock>> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stats(&self) -> RealtimeStats {
        RealtimeStats {
            idle: self.lock().len(),
            warm_claims: self.warm_claims.load(Ordering::Relaxed),
            cold_connects: self.cold_connects.load(Ordering::Relaxed),
            discarded: self.discarded.load(Ordering::Relaxed),
        }
    }

    async fn connect(&self) -> Result<WsStream, MtError> {
        if self.cfg.api_key.is_empty() {
            return Err(MtError::NotConfigured("realtime api key".into()));
        }
        let mut url = Url::parse(&self.cfg.base_url)
            .map_err(|e| MtError::Connect(format!("bad base_url: {e}")))?;
        if !url.query_pairs().any(|(k, _)| k == "model") {
            url.query_pairs_mut().append_pair("model", &self.cfg.model);
        }
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| MtError::Connect(e.to_string()))?;
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.cfg.api_key))
            .map_err(|_| MtError::Connect("api key is not a valid header value".into()))?;
        auth.set_sensitive(true);
        req.headers_mut().insert(AUTHORIZATION, auth);
        let t = Duration::from_millis(self.cfg.connect_timeout_ms);
        match tokio::time::timeout(t, tokio_tungstenite::connect_async(req)).await {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(e)) => Err(MtError::Connect(e.to_string())),
            Err(_) => Err(MtError::Timeout("realtime connect")),
        }
    }

    async fn open_one(&self) -> bool {
        match self.connect().await {
            Ok(ws) => {
                self.lock().push_back(Sock {
                    ws,
                    opened_at: Instant::now(),
                });
                true
            }
            Err(e) => {
                warn!(error = %e, "realtime translation socket failed");
                false
            }
        }
    }

    /// Dials `pool_size` sockets concurrently; returns how many succeeded.
    pub async fn warm_up(self: &Arc<Self>) -> usize {
        let r = futures::future::join_all((0..self.cfg.pool_size).map(|_| self.open_one())).await;
        r.into_iter().filter(|ok| *ok).count()
    }

    /// Background top-up to `pool_size`, counting dials in flight.
    pub fn refill(self: &Arc<Self>) {
        let have = self.lock().len() + self.inflight.load(Ordering::Relaxed);
        for _ in have..self.cfg.pool_size {
            self.inflight.fetch_add(1, Ordering::Relaxed);
            let me = self.clone();
            tokio::spawn(async move {
                me.open_one().await;
                me.inflight.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }

    fn usable(&self, s: &mut Sock) -> bool {
        s.opened_at.elapsed() < Duration::from_secs(self.cfg.max_socket_age_s)
            && still_open(&mut s.ws)
    }

    async fn claim(self: &Arc<Self>) -> Result<Sock, MtError> {
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
            debug!("discarded unusable realtime socket");
        }
        self.refill();
        if let Some(s) = found {
            self.warm_claims.fetch_add(1, Ordering::Relaxed);
            return Ok(s);
        }
        self.cold_connects.fetch_add(1, Ordering::Relaxed);
        Ok(Sock {
            ws: self.connect().await?,
            opened_at: Instant::now(),
        })
    }

    /// One isolated text response. The whole call (claim, send, stream) is bounded by
    /// `call.timeout`; on timeout or error the socket is dropped and the caller falls back.
    pub async fn translate(
        self: &Arc<Self>,
        system: &str,
        user: &str,
        call: &RealtimeCall,
    ) -> Result<RealtimeReply, MtError> {
        let deadline = tokio::time::Instant::now() + call.timeout;
        let mut sock = match tokio::time::timeout_at(deadline, self.claim()).await {
            Ok(r) => r?,
            Err(_) => return Err(MtError::Timeout("realtime claim")),
        };
        let request_id = uuid::Uuid::now_v7().to_string();
        let res = tokio::time::timeout_at(
            deadline,
            self.exchange(&mut sock.ws, system, user, call, &request_id),
        )
        .await;
        match res {
            Ok(Ok(reply)) => {
                if self.lock().len() < self.cfg.pool_size {
                    self.lock().push_back(sock);
                }
                Ok(reply)
            }
            Ok(Err(e)) => {
                self.refill();
                Err(e)
            }
            Err(_) => {
                self.refill();
                Err(MtError::Timeout("realtime"))
            }
        }
    }

    async fn exchange(
        &self,
        ws: &mut WsStream,
        system: &str,
        user: &str,
        call: &RealtimeCall,
        request_id: &str,
    ) -> Result<RealtimeReply, MtError> {
        let event = json!({
            "type": "response.create",
            "response": {
                "conversation": "none",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": user}],
                }],
                "instructions": system,
                "max_output_tokens": call.max_output_tokens,
                "metadata": {"request_id": request_id},
                "output_modalities": ["text"],
                "reasoning": {"effort": call.effort},
            }
        });
        ws.send(Message::Text(event.to_string().into()))
            .await
            .map_err(|e| MtError::Protocol(format!("send: {e}")))?;

        let mut response_id: Option<String> = None;
        let mut text = String::new();
        let mut first_token_at = None;
        loop {
            let msg = ws
                .next()
                .await
                .ok_or_else(|| MtError::Protocol("socket closed".into()))?
                .map_err(|e| MtError::Protocol(e.to_string()))?;
            let Message::Text(t) = msg else {
                if matches!(msg, Message::Close(_)) {
                    return Err(MtError::Protocol("socket closed".into()));
                }
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else {
                continue;
            };
            match v["type"].as_str().unwrap_or("") {
                "error" => return Err(MtError::Protocol(v["error"].to_string())),
                "response.created" => {
                    if v["response"]["metadata"]["request_id"].as_str() == Some(request_id) {
                        response_id = v["response"]["id"].as_str().map(str::to_string);
                    }
                }
                "response.output_text.delta"
                    if response_id.is_some()
                        && v["response_id"].as_str() == response_id.as_deref() =>
                {
                    if let Some(d) = v["delta"].as_str() {
                        first_token_at.get_or_insert_with(Instant::now);
                        text.push_str(d);
                    }
                }
                "response.done"
                    if response_id.is_some()
                        && v["response"]["id"].as_str() == response_id.as_deref() =>
                {
                    let status = v["response"]["status"].as_str().unwrap_or("");
                    if status != "completed" {
                        return Err(MtError::Protocol(format!("response ended with {status}")));
                    }
                    let out = text.trim().to_string();
                    if out.is_empty() {
                        return Err(MtError::Empty);
                    }
                    return Ok(RealtimeReply {
                        text: out,
                        first_token_at: first_token_at.unwrap_or_else(Instant::now),
                    });
                }
                _ => {}
            }
        }
    }
}
