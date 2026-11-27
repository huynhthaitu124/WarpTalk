//! `GET /copilot/sessions/{id}/audio`: the server-side STT fallback socket.
//!
//! Auth (all checked BEFORE the upgrade, so failures are plain HTTP statuses):
//! * `Authorization: Bearer <access token>` (native clients), or
//! * `?token=<ticket>` from `POST /copilot/sessions/{id}/audio-ticket` (browsers / WKWebView
//!   cannot set headers on a WebSocket). Tickets are single-use, session-bound and short lived.
//!
//! Protocol: see `domain::audio`. Backpressure is end to end: the loop feeds the provider
//! inline, so a slow provider stalls socket reads and TCP pushes back on the client (whose
//! bounded queue then drops its oldest audio); a provider that stalls longer than
//! `feed_timeout_ms` ends that channel's session with an error the client treats as a
//! provider failure.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::response::Response;
use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use warptalk_kernel::{AppError, AppResult, RoomId};
use warptalk_web::AuthUser;
use warptalk_web::extract::{Path, Query};

use super::AppState;
use crate::application::audio::{AudioConfig, AudioPermit};
use crate::application::ports::{ServerSttEvent, ServerSttSession};
use crate::application::service::{AudioContext, CopilotService, LanguagePrefs};
use crate::domain::audio::{
    ClientMsg, SAMPLE_RATE_HZ, ServerMsg, UtteranceClock, parse_audio_frame, server_segment_id,
};
use crate::domain::{CHANNEL_COUNT, Channel, LanguageMode, MAX_SEGMENT_TEXT_CHARS, Segment};

#[derive(Debug, Deserialize)]
pub struct AudioQuery {
    /// One-time ticket (see module docs).
    pub token: Option<String>,
}

/// `Some` when a valid bearer (header, or `access_token` on an upgrade) is present.
pub struct MaybeAuth(pub Option<AuthUser>);

impl FromRequestParts<AppState> for MaybeAuth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(AuthUser::from_request_parts(parts, state).await.ok()))
    }
}

pub async fn audio_ws(
    State(s): State<AppState>,
    Path(id): Path<RoomId>,
    Query(q): Query<AudioQuery>,
    MaybeAuth(auth): MaybeAuth,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    let user = match (auth, q.token) {
        (Some(a), _) => a.user_id(),
        (None, Some(t)) => s.service.redeem_audio_ticket(id, &t).await?,
        (None, None) => {
            return Err(AppError::unauthorized(
                "missing bearer token or audio ticket",
            ));
        }
    };
    let (ctx, permit) = s.service.open_audio(id, user).await?;
    let cfg = s.service.audio_config();
    let service = s.service.clone();
    Ok(ws
        .max_message_size(cfg.max_frame_bytes.max(64 * 1024))
        .max_frame_size(cfg.max_frame_bytes.max(64 * 1024))
        .on_upgrade(move |socket| run(service, cfg, ctx, permit, socket)))
}

struct Chan {
    session: Option<Box<dyn ServerSttSession>>,
    /// Samples fed since the last commit.
    fed: u64,
    pending: VecDeque<UtteranceClock>,
    /// Clock for provider events that arrive with no commit outstanding.
    orphan: Option<UtteranceClock>,
}

impl Chan {
    fn new() -> Self {
        Self {
            session: None,
            fed: 0,
            pending: VecDeque::new(),
            orphan: None,
        }
    }
}

struct Conn {
    service: Arc<CopilotService>,
    cfg: AudioConfig,
    ctx: AudioContext,
    out: mpsc::Sender<ServerMsg>,
    ev_tx: Option<mpsc::Sender<(Channel, ServerSttEvent)>>,
    chans: [Chan; CHANNEL_COUNT],
    prefs: LanguagePrefs,
    run: String,
    credits_warned: bool,
    bad_messages: u32,
    fatal: Option<String>,
}

fn random_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Conn {
    /// Important messages wait for room (the client slowing down is backpressure); a client
    /// that stops reading for 10 s is cut off.
    async fn send(&mut self, m: ServerMsg) {
        if tokio::time::timeout(Duration::from_secs(10), self.out.send(m))
            .await
            .is_err()
        {
            self.fatal = Some("client_not_reading".into());
        }
    }

    /// Droppable (partials).
    fn try_send(&self, m: ServerMsg) {
        let _ = self.out.try_send(m);
    }

    async fn error(
        &mut self,
        code: &str,
        message: impl Into<String>,
        ch: Option<Channel>,
        fatal: bool,
    ) {
        self.send(ServerMsg::Error {
            code: code.into(),
            message: message.into(),
            channel: ch,
            fatal,
        })
        .await;
        if fatal {
            self.fatal = Some(code.into());
        }
    }

    async fn ensure_session(&mut self, ch: Channel) -> bool {
        if self.chans[ch.index()].session.is_some() {
            return true;
        }
        let params = self
            .service
            .audio_stt_params(&self.ctx, ch, &self.prefs)
            .await;
        let Some(audio) = self.service.audio() else {
            return false;
        };
        let stt = audio.stt.clone();
        let (tx, mut rx) = mpsc::channel(64);
        let Some(fwd) = self.ev_tx.clone() else {
            return false;
        };
        let opened = tokio::time::timeout(
            Duration::from_millis(self.cfg.open_timeout_ms),
            stt.open(params, tx),
        )
        .await;
        match opened {
            Ok(Ok(session)) => {
                tokio::spawn(async move {
                    while let Some(ev) = rx.recv().await {
                        if fwd.send((ch, ev)).await.is_err() {
                            break;
                        }
                    }
                });
                self.chans[ch.index()].session = Some(session);
                true
            }
            other => {
                let msg = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "provider open timed out".into(),
                };
                tracing::warn!(session = %self.ctx.session, channel = ch.as_str(), error = %msg, "server stt open failed");
                self.error("stt_unavailable", msg, Some(ch), false).await;
                false
            }
        }
    }

    async fn on_audio(&mut self, bytes: &[u8]) {
        let (ch, pcm) = match parse_audio_frame(bytes, self.cfg.max_frame_bytes) {
            Ok(v) => v,
            Err(e) => {
                self.bad("bad_frame", e.to_string()).await;
                return;
            }
        };
        let c = &self.chans[ch.index()];
        let max = self.cfg.max_utterance_audio_ms * u64::from(SAMPLE_RATE_HZ) / 1000;
        if c.fed + pcm.len() as u64 > max {
            self.error(
                "utterance_too_long",
                "too much audio without a commit; send `commit`",
                Some(ch),
                false,
            )
            .await;
            return;
        }
        if !self.ensure_session(ch).await {
            return;
        }
        let timeout = Duration::from_millis(self.cfg.feed_timeout_ms);
        let n = pcm.len() as u64;
        let c = &mut self.chans[ch.index()];
        let res = match c.session.as_mut() {
            Some(sess) => tokio::time::timeout(timeout, sess.feed(&pcm)).await,
            None => return,
        };
        match res {
            Ok(Ok(())) => c.fed += n,
            Ok(Err(e)) => self.session_failed(ch, e.to_string()).await,
            Err(_) => self.session_failed(ch, "provider stalled".into()).await,
        }
    }

    /// Drop the channel's provider session; the client sees a provider failure and reacts.
    async fn session_failed(&mut self, ch: Channel, msg: String) {
        tracing::warn!(session = %self.ctx.session, channel = ch.as_str(), error = %msg, "server stt session failed");
        let c = &mut self.chans[ch.index()];
        c.fed = 0;
        let pending = std::mem::take(&mut c.pending);
        let sess = c.session.take();
        if let Some(s) = sess {
            tokio::spawn(async move { s.close().await });
        }
        for u in pending {
            self.send(ServerMsg::UtteranceDone {
                channel: ch,
                seq: u.seq,
                ok: false,
            })
            .await;
        }
        self.error("stt_failed", msg, Some(ch), false).await;
    }

    async fn bad(&mut self, code: &str, msg: String) {
        self.bad_messages += 1;
        let fatal = self.bad_messages > self.cfg.max_bad_messages;
        self.error(code, msg, None, fatal).await;
    }

    async fn on_text(&mut self, text: &str) {
        let msg: ClientMsg = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                self.bad("bad_message", e.to_string()).await;
                return;
            }
        };
        match msg {
            ClientMsg::Ping => self.send(ServerMsg::Pong).await,
            ClientMsg::Hello {
                run_id,
                language_mode,
                languages,
                vocabulary,
            } => {
                if self.chans.iter().any(|c| c.session.is_some()) {
                    self.error("hello_too_late", "hello must precede audio", None, false)
                        .await;
                    return;
                }
                if let Some(r) = run_id.filter(|r| {
                    !r.is_empty()
                        && r.len() <= 64
                        && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                }) {
                    self.run = r;
                }
                self.prefs = LanguagePrefs {
                    mode: language_mode.as_deref().and_then(LanguageMode::parse),
                    languages,
                    vocabulary,
                };
            }
            ClientMsg::Discard { channel } => {
                let c = &mut self.chans[channel.index()];
                c.fed = 0;
                if let Some(s) = c.session.as_mut()
                    && let Err(e) = s.discard().await
                {
                    self.session_failed(channel, e.to_string()).await;
                }
            }
            ClientMsg::CloseChannel { channel } => {
                if let Some(s) = self.chans[channel.index()].session.take() {
                    // Drains in the background; its events still reach `ev_rx`.
                    tokio::spawn(async move { s.close().await });
                }
                self.chans[channel.index()].fed = 0;
            }
            ClientMsg::Commit {
                channel,
                seq,
                start_ms,
                end_ms,
                speaker,
            } => {
                let c = &mut self.chans[channel.index()];
                if c.fed == 0 || c.session.is_none() {
                    self.send(ServerMsg::UtteranceDone {
                        channel,
                        seq,
                        ok: false,
                    })
                    .await;
                    self.error(
                        "empty_utterance",
                        "commit without audio",
                        Some(channel),
                        false,
                    )
                    .await;
                    return;
                }
                c.fed = 0;
                let mut clock = UtteranceClock::new(seq, start_ms, end_ms);
                clock.speaker = crate::domain::clean_speaker_name(speaker.as_deref());
                c.pending.push_back(clock);
                let res = match c.session.as_mut() {
                    Some(s) => s.commit().await,
                    None => return,
                };
                if let Err(e) = res {
                    self.session_failed(channel, e.to_string()).await;
                }
            }
        }
    }

    async fn on_event(&mut self, ch: Channel, ev: ServerSttEvent) {
        match ev {
            ServerSttEvent::Segment {
                text,
                language,
                confidence,
                early,
            } => self.on_segment(ch, text, language, confidence, early).await,
            ServerSttEvent::UtteranceDone { ok } => {
                let c = &mut self.chans[ch.index()];
                let done = c.pending.pop_front();
                c.orphan = None;
                self.try_send(ServerMsg::Partial {
                    channel: ch,
                    text: String::new(),
                });
                if let Some(u) = done {
                    self.send(ServerMsg::UtteranceDone {
                        channel: ch,
                        seq: u.seq,
                        ok,
                    })
                    .await;
                }
                if !ok {
                    self.error(
                        "stt_failed",
                        "transcription failed for an utterance",
                        Some(ch),
                        false,
                    )
                    .await;
                }
            }
        }
    }

    async fn on_segment(
        &mut self,
        ch: Channel,
        text: String,
        language: Option<String>,
        confidence: Option<f32>,
        early: bool,
    ) {
        let text: String = text.trim().chars().take(MAX_SEGMENT_TEXT_CHARS).collect();
        if text.is_empty() {
            return;
        }
        let est = self.cfg.est_ms_per_char;
        let c = &mut self.chans[ch.index()];
        let clock = match c.pending.front_mut() {
            Some(u) => u,
            None => c
                .orphan
                .get_or_insert_with(|| UtteranceClock::new(u64::MAX, 0, 0)),
        };
        let (start, end) = clock.next(&text, early, est);
        let (seq, n, partial) = (clock.seq, clock.emitted - 1, clock.text.clone());
        let speaker_name = clock.speaker.clone();
        let id = server_segment_id(&self.run, ch, seq, n);
        let language = language
            .filter(|l| crate::domain::valid_language(l))
            .unwrap_or_else(|| "und".into());
        let seg = Segment {
            client_segment_id: id.clone(),
            channel: ch,
            text: text.clone(),
            language: language.clone(),
            start_ms: start,
            end_ms: end,
            confidence: confidence.map(|c| f64::from(c).clamp(0.0, 1.0)),
            is_final: true,
            speaker_name,
        };
        let confidence = seg.confidence;
        match self.service.ingest_audio_segment(&self.ctx, seg).await {
            Ok(out) => {
                let (segment_id, duplicate) = out
                    .segments
                    .first()
                    .map(|(_, sid, d)| (*sid, *d))
                    .unwrap_or_default();
                self.send(ServerMsg::Segment {
                    channel: ch,
                    client_segment_id: id,
                    segment_id,
                    text,
                    language,
                    start_ms: start,
                    end_ms: end,
                    confidence,
                    early,
                    duplicate,
                    insufficient_credits: out.insufficient_credits,
                })
                .await;
                if out.insufficient_credits && !self.credits_warned {
                    self.credits_warned = true;
                    self.error(
                        "insufficient_credits",
                        "AI credits are exhausted: the transcript is kept, suggestions are suspended",
                        None,
                        false,
                    )
                    .await;
                }
            }
            Err(AppError::Conflict { .. }) => {
                self.error(
                    "session_ended",
                    "the session is no longer in progress",
                    None,
                    true,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(session = %self.ctx.session, error = %e, "audio segment ingest failed");
                self.error(
                    "ingest_failed",
                    "could not store the segment",
                    Some(ch),
                    false,
                )
                .await;
            }
        }
        if early {
            self.try_send(ServerMsg::Partial {
                channel: ch,
                text: partial,
            });
        }
    }
}

async fn next_client_message(
    rd: &mut SplitStream<WebSocket>,
    idle: Duration,
) -> Result<Option<Message>, &'static str> {
    match tokio::time::timeout(idle, rd.next()).await {
        Err(_) => Err("idle_timeout"),
        Ok(None) | Ok(Some(Err(_))) => Ok(None),
        Ok(Some(Ok(m))) => Ok(Some(m)),
    }
}

async fn run(
    service: Arc<CopilotService>,
    cfg: AudioConfig,
    ctx: AudioContext,
    _permit: AudioPermit,
    socket: WebSocket,
) {
    let (mut sink, mut rd) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerMsg>(cfg.out_queue);
    let writer = tokio::spawn(async move {
        let mut close_reason = String::new();
        while let Some(m) = out_rx.recv().await {
            if let ServerMsg::Closing { reason } = &m {
                close_reason = reason.clone();
            }
            let Ok(json) = serde_json::to_string(&m) else {
                continue;
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code: 1000,
                reason: close_reason.chars().take(100).collect::<String>().into(),
            })))
            .await;
    });
    let (ev_tx, mut ev_rx) = mpsc::channel::<(Channel, ServerSttEvent)>(256);
    let run = random_id();
    let mut conn = Conn {
        service,
        cfg: cfg.clone(),
        ctx,
        out: out_tx,
        ev_tx: Some(ev_tx),
        chans: std::array::from_fn(|_| Chan::new()),
        prefs: LanguagePrefs::default(),
        run: run.clone(),
        credits_warned: false,
        bad_messages: 0,
        fatal: None,
    };
    conn.send(ServerMsg::Ready {
        connection_id: run,
        sample_rate: SAMPLE_RATE_HZ,
        max_frame_bytes: cfg.max_frame_bytes,
        max_session_secs: cfg.max_session_secs,
    })
    .await;

    let deadline = tokio::time::sleep(Duration::from_secs(cfg.max_session_secs));
    tokio::pin!(deadline);
    let idle = Duration::from_secs(cfg.idle_timeout_secs);
    let mut reason = String::from("client_closed");
    loop {
        if let Some(f) = conn.fatal.take() {
            reason = f;
            break;
        }
        tokio::select! {
            biased;
            _ = &mut deadline => { reason = "max_session_duration".into(); break; }
            Some((ch, ev)) = ev_rx.recv() => conn.on_event(ch, ev).await,
            msg = next_client_message(&mut rd, idle) => match msg {
                Err(r) => { reason = r.into(); break; }
                Ok(None) | Ok(Some(Message::Close(_))) => break,
                Ok(Some(Message::Binary(b))) => conn.on_audio(&b).await,
                Ok(Some(Message::Text(t))) => conn.on_text(t.as_str()).await,
                Ok(Some(_)) => {}
            },
        }
    }

    // Finish in-flight transcripts (their segments are still ingested), then say goodbye.
    let drain = Duration::from_millis(cfg.drain_timeout_ms);
    let sessions: Vec<_> = conn
        .chans
        .iter_mut()
        .filter_map(|c| c.session.take())
        .collect();
    let closer = tokio::spawn(async move {
        for s in sessions {
            s.close().await;
        }
    });
    // Drop our sender so the receiver ends once every provider session has closed.
    conn.ev_tx = None;
    let _ = tokio::time::timeout(drain, async {
        while let Some((ch, ev)) = ev_rx.recv().await {
            conn.on_event(ch, ev).await;
        }
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(1), closer).await;
    conn.send(ServerMsg::Closing { reason }).await;
    drop(conn);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}
