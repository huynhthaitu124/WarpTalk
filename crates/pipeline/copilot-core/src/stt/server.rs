//! Server-side STT fallback client: streams speech audio to the API's audio socket
//! (`GET /api/v1/copilot/sessions/{id}/audio`, protocol in `warptalk-copilot::domain::audio`)
//! and turns the server's `segment` / `utterance_done` messages back into [`SttEvent`]s, so the
//! rest of the engine treats it like any other [`SpeechToText`].
//!
//! * One socket carries both channels (binary frame = 1 channel byte + PCM16 LE 16 kHz mono);
//!   utterance ends go as `commit` text frames with their position on the session timeline
//!   (via the [`TimingBoard`]). The bearer travels in `Authorization` (a native client can set
//!   it; browsers use the ticket flow on the server side, see the API docs).
//! * The **server ingests** the segments (same idempotent path as `POST /segments`), so the
//!   channel worker must NOT post them to the sink: it knows by the session's route.
//! * The socket is shared by the channel sessions and reconnected transparently when the
//!   server closes it (max duration, restart); commits that were in flight on a lost socket
//!   finish as failed utterances. Provider-level errors of a channel (`error` with `channel`)
//!   end only that channel's session so the worker reacts like to any STT failure.
//! * Account-level notices (`insufficient_credits`, `session_ended`) go to the
//!   [`NoticeBoard`], which the engine turns into `Error` events.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttError, SttEvent, SttSegment, SttSession,
};

use crate::config::{LanguageConfig, ServerSttConfig};
use crate::error::CopilotError;
use crate::types::Channel;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Samples per binary frame (1 s); also capped by the server's advertised frame limit.
const FRAME_SAMPLES: usize = 16_000;

/// Position of each committed utterance on the session timeline: the worker writes it just
/// before `commit`, the server session reads it (the `SttSession` trait has no time fields).
#[derive(Default)]
pub struct TimingBoard {
    items: Mutex<HashMap<(Channel, u64), (u64, u64)>>,
}

impl TimingBoard {
    pub fn set(&self, ch: Channel, seq: u64, start_ms: u64, end_ms: u64) {
        let mut m = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        if m.len() > 256 {
            m.clear(); // never grows unbounded if a backend never reads
        }
        m.insert((ch, seq), (start_ms, end_ms));
    }

    pub fn take(&self, ch: Channel, seq: u64) -> Option<(u64, u64)> {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(ch, seq))
    }
}

/// Account-level messages from the server for the engine to surface (`code`, `message`).
#[derive(Default)]
pub struct NoticeBoard {
    items: Mutex<VecDeque<(String, String)>>,
}

impl NoticeBoard {
    pub fn push(&self, code: impl Into<String>, message: impl Into<String>) {
        let mut q = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() < 32 {
            q.push_back((code.into(), message.into()));
        }
    }

    pub fn drain(&self) -> Vec<(String, String)> {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum ServerMsg {
    Ready {
        #[serde(default)]
        max_frame_bytes: Option<usize>,
    },
    Segment {
        channel: String,
        text: String,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        confidence: Option<f32>,
        #[serde(default)]
        early: bool,
        #[serde(default)]
        insufficient_credits: bool,
    },
    UtteranceDone {
        channel: String,
        ok: bool,
    },
    Error {
        code: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        fatal: bool,
    },
    Closing {
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Other,
}

fn ch_of(s: &str) -> Option<Channel> {
    match s {
        "me" => Some(Channel::Me),
        "others" => Some(Channel::Others),
        _ => None,
    }
}

struct Route {
    sink: mpsc::Sender<SttEvent>,
    /// Set when the server reported a provider error for this channel.
    failed: Arc<AtomicBool>,
    outstanding: u64,
}

struct Conn {
    writer: tokio::sync::Mutex<SplitSink<WsStream, Message>>,
    routes: Mutex<[Option<Route>; 2]>,
    dead: AtomicBool,
    max_frame_samples: AtomicU64,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Conn {
    fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    fn with_route<R>(&self, ch: Channel, f: impl FnOnce(&mut Route) -> R) -> Option<R> {
        let mut g = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
        g[ch.index()].as_mut().map(f)
    }

    fn sink_of(&self, ch: Channel) -> Option<mpsc::Sender<SttEvent>> {
        self.with_route(ch, |r| r.sink.clone())
    }

    /// Every outstanding commit of a lost socket finishes as failed.
    async fn fail_outstanding(&self) {
        let sinks: Vec<(mpsc::Sender<SttEvent>, u64)> = {
            let mut g = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
            g.iter_mut()
                .flatten()
                .map(|r| (r.sink.clone(), std::mem::take(&mut r.outstanding)))
                .collect()
        };
        for (sink, n) in sinks {
            for _ in 0..n {
                let _ = sink.send(SttEvent::UtteranceDone { ok: false }).await;
            }
        }
    }
}

struct Inner {
    cfg: ServerSttConfig,
    url: String,
    bearer: Mutex<String>,
    hello: serde_json::Value,
    timing: Arc<TimingBoard>,
    notices: Arc<NoticeBoard>,
    conn: tokio::sync::Mutex<Option<Arc<Conn>>>,
    connects: AtomicU64,
}

/// See the module docs.
pub struct ServerAudioStt {
    inner: Arc<Inner>,
}

impl ServerAudioStt {
    /// `base_url`: `http(s)://host[:port]` of the API.
    pub fn new(
        cfg: ServerSttConfig,
        lang: &LanguageConfig,
        base_url: &str,
        session_id: &str,
        bearer: impl Into<String>,
        timing: Arc<TimingBoard>,
        notices: Arc<NoticeBoard>,
    ) -> Result<Self, CopilotError> {
        let base = base_url.trim_end_matches('/');
        let ws_base = if let Some(r) = base.strip_prefix("https://") {
            format!("wss://{r}")
        } else if let Some(r) = base.strip_prefix("http://") {
            format!("ws://{r}")
        } else {
            base.to_owned()
        };
        let url = format!("{ws_base}{}", cfg.path_template.replace("{id}", session_id));
        url::Url::parse(&url).map_err(|e| CopilotError::Config(format!("server stt url: {e}")))?;
        let hello = json!({
            "type": "hello",
            "runId": uuid::Uuid::new_v4().to_string(),
            "languageMode": lang.mode.as_str(),
            "languages": lang.languages,
            "vocabulary": lang.vocabulary,
        });
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                url,
                bearer: Mutex::new(bearer.into()),
                hello,
                timing,
                notices,
                conn: tokio::sync::Mutex::new(None),
                connects: AtomicU64::new(0),
            }),
        })
    }

    pub fn set_bearer(&self, bearer: impl Into<String>) {
        *self
            .inner
            .bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = bearer.into();
    }

    /// Sockets opened so far (tests / diagnostics).
    pub fn connects(&self) -> u64 {
        self.inner.connects.load(Ordering::Relaxed)
    }
}

impl Inner {
    async fn connect(self: &Arc<Self>) -> Result<Arc<Conn>, SttError> {
        let bearer = self
            .bearer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut req = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| SttError::Connect(e.to_string()))?;
        let mut auth = HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| SttError::Connect("bearer is not a valid header value".into()))?;
        auth.set_sensitive(true);
        req.headers_mut().insert(AUTHORIZATION, auth);
        let t = Duration::from_millis(self.cfg.connect_timeout_ms);
        let ws = match tokio::time::timeout(t, tokio_tungstenite::connect_async(req)).await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) => {
                // Same shape as the token path ("http 401") so the worker maps auth errors.
                return Err(SttError::Connect(format!("http {}", r.status().as_u16())));
            }
            Ok(Err(e)) => return Err(SttError::Connect(e.to_string())),
            Err(_) => return Err(SttError::Timeout("connect".into())),
        };
        self.connects.fetch_add(1, Ordering::Relaxed);
        let (mut writer, rd) = ws.split();
        writer
            .send(Message::Text(self.hello.to_string().into()))
            .await
            .map_err(|e| SttError::Connect(e.to_string()))?;
        let conn = Arc::new(Conn {
            writer: tokio::sync::Mutex::new(writer),
            routes: Mutex::new([None, None]),
            dead: AtomicBool::new(false),
            max_frame_samples: AtomicU64::new(FRAME_SAMPLES as u64),
            reader: Mutex::new(None),
        });
        let h = tokio::spawn(read_loop(
            rd,
            conn.clone(),
            self.notices.clone(),
            Duration::from_millis(self.cfg.event_timeout_ms),
        ));
        *conn.reader.lock().unwrap_or_else(PoisonError::into_inner) = Some(h);
        Ok(conn)
    }

    /// The live shared socket, connecting if there is none.
    async fn live_conn(self: &Arc<Self>) -> Result<Arc<Conn>, SttError> {
        let mut g = self.conn.lock().await;
        if let Some(c) = g.as_ref()
            && !c.is_dead()
        {
            return Ok(c.clone());
        }
        let c = self.connect().await?;
        *g = Some(c.clone());
        Ok(c)
    }
}

async fn read_loop(
    mut rd: SplitStream<WsStream>,
    conn: Arc<Conn>,
    notices: Arc<NoticeBoard>,
    event_timeout: Duration,
) {
    loop {
        let waiting = conn
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .flatten()
            .any(|r| r.outstanding > 0);
        let next = if waiting {
            match tokio::time::timeout(event_timeout, rd.next()).await {
                Ok(m) => m,
                Err(_) => {
                    warn!("server stt: no message while a commit is outstanding");
                    break;
                }
            }
        } else {
            rd.next().await
        };
        let text = match next {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(_)) => continue,
        };
        let msg: ServerMsg = match serde_json::from_str(text.as_str()) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "unparseable server stt message ignored");
                continue;
            }
        };
        match msg {
            ServerMsg::Ready { max_frame_bytes } => {
                if let Some(b) = max_frame_bytes {
                    let samples = (b.saturating_sub(1) / 2).max(320);
                    conn.max_frame_samples
                        .store(samples.min(FRAME_SAMPLES) as u64, Ordering::Relaxed);
                }
            }
            ServerMsg::Segment {
                channel,
                text,
                language,
                confidence,
                early,
                insufficient_credits,
            } => {
                if insufficient_credits {
                    notices.push(
                        "insufficient_credits",
                        "AI credits are exhausted: the transcript is kept, suggestions are suspended",
                    );
                }
                if let Some(sink) = ch_of(&channel).and_then(|c| conn.sink_of(c)) {
                    let seg = SttSegment::new(
                        text,
                        language.unwrap_or_else(|| "und".into()),
                        confidence,
                        early,
                    );
                    let _ = sink.send(SttEvent::Segment(seg)).await;
                }
            }
            ServerMsg::UtteranceDone { channel, ok } => {
                let Some(ch) = ch_of(&channel) else { continue };
                let sink = conn.with_route(ch, |r| {
                    r.outstanding = r.outstanding.saturating_sub(1);
                    r.sink.clone()
                });
                if let Some(s) = sink {
                    let _ = s.send(SttEvent::UtteranceDone { ok }).await;
                }
            }
            ServerMsg::Error {
                code,
                message,
                channel,
                fatal,
            } => {
                match channel.as_deref().and_then(ch_of) {
                    Some(ch) => {
                        // A provider failure of this channel: its session ends (the worker
                        // reacts as to any STT failure); other channels keep running.
                        conn.with_route(ch, |r| r.failed.store(true, Ordering::Relaxed));
                    }
                    None => notices.push(code, message),
                }
                if fatal {
                    break;
                }
            }
            ServerMsg::Closing { reason } => {
                debug!(%reason, "server stt closing");
                break;
            }
            ServerMsg::Other => {}
        }
    }
    conn.dead.store(true, Ordering::Relaxed);
    conn.fail_outstanding().await;
}

#[async_trait]
impl SpeechToText for ServerAudioStt {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        let ch = ch_of(&params.speaker)
            .ok_or_else(|| SttError::Other(format!("unknown channel {}", params.speaker)))?;
        let conn = self.inner.live_conn().await?;
        let failed = Arc::new(AtomicBool::new(false));
        {
            let mut g = conn.routes.lock().unwrap_or_else(PoisonError::into_inner);
            g[ch.index()] = Some(Route {
                sink: sink.clone(),
                failed: failed.clone(),
                outstanding: 0,
            });
        }
        Ok(Box::new(Session {
            inner: self.inner.clone(),
            ch,
            sink,
            conn,
            failed,
        }))
    }
}

struct Session {
    inner: Arc<Inner>,
    ch: Channel,
    sink: mpsc::Sender<SttEvent>,
    conn: Arc<Conn>,
    failed: Arc<AtomicBool>,
}

impl Session {
    /// The socket to use now: this session's, or (when the server closed it) a fresh one this
    /// session re-registers on.
    async fn usable(&mut self) -> Result<Arc<Conn>, SttError> {
        if self.failed.load(Ordering::Relaxed) {
            return Err(SttError::Other(
                "server stt reported a channel failure".into(),
            ));
        }
        if self.conn.is_dead() {
            let c = self.inner.live_conn().await?;
            c.routes.lock().unwrap_or_else(PoisonError::into_inner)[self.ch.index()] =
                Some(Route {
                    sink: self.sink.clone(),
                    failed: self.failed.clone(),
                    outstanding: 0,
                });
            self.conn = c;
        }
        Ok(self.conn.clone())
    }

    async fn send(&self, conn: &Conn, m: Message) -> Result<(), SttError> {
        let t = Duration::from_millis(self.inner.cfg.send_timeout_ms);
        let mut w = conn.writer.lock().await;
        match tokio::time::timeout(t, w.send(m)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                conn.dead.store(true, Ordering::Relaxed);
                Err(SttError::Other(format!("ws send: {e}")))
            }
            Err(_) => {
                conn.dead.store(true, Ordering::Relaxed);
                Err(SttError::Timeout("ws send".into()))
            }
        }
    }

    async fn send_audio(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        let conn = self.usable().await?;
        let max = conn.max_frame_samples.load(Ordering::Relaxed).max(320) as usize;
        for chunk in pcm_16k.chunks(max.min(FRAME_SAMPLES)) {
            let mut frame = Vec::with_capacity(1 + chunk.len() * 2);
            frame.push(self.ch.index() as u8);
            for s in chunk {
                let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                frame.extend_from_slice(&v.to_le_bytes());
            }
            self.send(&conn, Message::Binary(frame.into())).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl SttSession for Session {
    async fn feed(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        self.send_audio(pcm_16k).await
    }

    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError> {
        if req.streamed_samples == 0 {
            let audio = req.utterance.audio.clone();
            self.send_audio(&audio).await?;
        }
        let conn = self.usable().await?;
        let seq = req.utterance.seq;
        let (start, end) = self
            .inner
            .timing
            .take(self.ch, seq)
            .unwrap_or((0, u64::from(req.utterance.total_ms)));
        conn.with_route(self.ch, |r| r.outstanding += 1);
        let msg = json!({
            "type": "commit",
            "channel": self.ch.as_str(),
            "seq": seq,
            "startMs": start,
            "endMs": end.max(start),
        });
        if let Err(e) = self
            .send(&conn, Message::Text(msg.to_string().into()))
            .await
        {
            conn.with_route(self.ch, |r| r.outstanding = r.outstanding.saturating_sub(1));
            return Err(e);
        }
        Ok(())
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        let conn = self.usable().await?;
        let msg = json!({ "type": "discard", "channel": self.ch.as_str() });
        self.send(&conn, Message::Text(msg.to_string().into()))
            .await
    }

    async fn close(self: Box<Self>) {
        // Let in-flight commits finish (the shared socket stays open for the other channel).
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.inner.cfg.event_timeout_ms);
        while tokio::time::Instant::now() < deadline {
            let n = self
                .conn
                .with_route(self.ch, |r| r.outstanding)
                .unwrap_or(0);
            if n == 0 || self.conn.is_dead() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut g = self
            .conn
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Only clear the route if it is still ours (a new session may have replaced it).
        if g[self.ch.index()]
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(&r.failed, &self.failed))
        {
            g[self.ch.index()] = None;
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(g) = self.conn.try_lock()
            && let Some(c) = g.as_ref()
            && let Some(h) = c
                .reader
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        {
            h.abort();
        }
    }
}
