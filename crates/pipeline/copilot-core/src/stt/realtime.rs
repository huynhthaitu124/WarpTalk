//! OpenAI Realtime transcription over WebSocket, authenticated with a server-issued
//! **ephemeral token** (ADR 0001 item 4) instead of a platform API key.
//!
//! This is the client side of the protocol from `warptalk-stt` (wire types, `TurnMachine`
//! sentence splitting, pcm16 24 kHz encoding are reused from there); only connection setup
//! differs: the token comes from a [`TokenManager`] (refreshed before expiry), the URL from the
//! token response's `websocketUrl` (fallback: configured default), and there is no warm pool
//! (the copilot keeps one session per channel for the whole meeting and rotates it at
//! utterance boundaries).
//!
//! UNVERIFIED (as in `warptalk-stt`): that OpenAI accepts the ephemeral client secret as
//! `Authorization: Bearer` on `wss://api.openai.com/v1/realtime?intent=transcription`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use url::Url;
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttError, SttEvent, SttMode, SttSegment, SttSession,
};
use warptalk_stt::protocol::{ClientEvent, ServerEvent, SessionConfig, TranscriptionConfig};
use warptalk_stt::turns::{Out, TurnMachine};
use warptalk_stt::{LogprobFilter, encode_pcm16_24k};

use crate::config::SttConfig;
use crate::token::TokenManager;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Samples (16 kHz) per `append` message: 1 s, a multiple of the 3:2 resampling ratio.
const APPEND_CHUNK_SAMPLES: usize = 16_000;
const NEXT_GEN_MODELS: &[&str] = &["gpt-transcribe", "gpt-live-transcribe"];

/// Recent transcript kept as the transcription prompt: at most this many characters.
const CONTEXT_MAX_CHARS: usize = 300;

/// The meeting's latest transcript, shared by every session of this STT (both channels).
/// Sent as the transcription `prompt` (the model reads it as the text that came before), so a
/// tech discussion keeps its vocabulary: "mobile" instead of "mùa bò", "test" instead of "tết".
#[derive(Default)]
struct RecentContext {
    text: std::sync::Mutex<String>,
}

impl RecentContext {
    fn push(&self, sentence: &str) {
        let mut t = self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !t.is_empty() {
            t.push(' ');
        }
        t.push_str(sentence.trim());
        *t = tail_words(&t, CONTEXT_MAX_CHARS);
    }

    fn get(&self) -> String {
        self.text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The last `max` characters of `s`, starting at a word boundary.
fn tail_words(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let cut: String = s.chars().skip(n - max).collect();
    match cut.split_once(char::is_whitespace) {
        Some((_, rest)) => rest.trim_start().to_string(),
        None => cut,
    }
}

pub struct EphemeralRealtimeStt {
    cfg: SttConfig,
    tokens: Arc<TokenManager>,
    context: Arc<RecentContext>,
}

impl EphemeralRealtimeStt {
    pub fn new(cfg: SttConfig, tokens: Arc<TokenManager>) -> Self {
        Self {
            cfg,
            tokens,
            context: Arc::default(),
        }
    }

    fn session_config(&self, model: &str, lang: Option<&str>) -> SessionConfig {
        let next_gen = NEXT_GEN_MODELS.contains(&model);
        let mut t = TranscriptionConfig {
            model: model.to_string(),
            ..TranscriptionConfig::default()
        };
        // ISO-639-1 only: the realtime API rejects region tags like "en-US".
        let lang = lang.map(warptalk_stt::base_language);
        if let Some(l) = lang.as_deref().filter(|l| *l != "auto") {
            if next_gen {
                t.languages.push(l.to_string());
                if l != "en" {
                    t.languages.push("en".into());
                }
            } else {
                t.language = Some(l.to_string());
            }
        }
        let logprobs = self.cfg.request_logprobs.unwrap_or(!next_gen);
        SessionConfig::transcription(t, logprobs)
    }

    async fn connect(&self, url: &str, token: &str) -> Result<WsStream, SttError> {
        let mut url = Url::parse(url).map_err(|e| SttError::Connect(format!("bad url: {e}")))?;
        if !url.query_pairs().any(|(k, _)| k == "intent") {
            url.query_pairs_mut().append_pair("intent", "transcription");
        }
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| SttError::Connect(e.to_string()))?;
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| SttError::Connect("token is not a valid header value".into()))?;
        auth.set_sensitive(true);
        req.headers_mut().insert(AUTHORIZATION, auth);
        let t = Duration::from_millis(self.cfg.connect_timeout_ms);
        match tokio::time::timeout(t, tokio_tungstenite::connect_async(req)).await {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(e)) => Err(SttError::Connect(e.to_string())),
            Err(_) => Err(SttError::Timeout("connect".into())),
        }
    }
}

enum ReaderCmd {
    Commit(Option<String>),
    Shutdown,
}

struct Session {
    writer: SplitSink<WsStream, Message>,
    cmd: mpsc::UnboundedSender<ReaderCmd>,
    reader: JoinHandle<()>,
    dead: Arc<AtomicBool>,
    lang: Option<String>,
    event_timeout: Duration,
    /// The session's transcription config, re-sent with the recent transcript as its prompt
    /// (a `session.update` that names only the prompt resets `languages`).
    base_session: Option<serde_json::Value>,
    base_prompt: String,
    sent_prompt: String,
    context: Arc<RecentContext>,
}

#[async_trait]
impl SpeechToText for EphemeralRealtimeStt {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        let tok = self
            .tokens
            .get()
            .await
            .map_err(|e| SttError::Connect(e.to_string()))?;
        let url = tok
            .websocket_url
            .clone()
            .unwrap_or_else(|| self.cfg.default_websocket_url.clone());
        let ws = self.connect(&url, &tok.token).await?;
        let (mut writer, reader_half) = ws.split();
        let lang = params.lang_hint.or_else(|| self.cfg.language_hint.clone());
        // A server-issued token already carries the transcription config.
        let base_session = match &tok.session_config {
            Some(v) => Some(v.clone()),
            None => {
                let session = self.session_config(&tok.model, lang.as_deref());
                send_event(
                    &mut writer,
                    &ClientEvent::SessionUpdate {
                        session: session.clone(),
                    },
                )
                .await?;
                serde_json::to_value(&session).ok()
            }
        };
        let base_prompt = base_session
            .as_ref()
            .and_then(transcription_node)
            .and_then(|t| t.get("prompt"))
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let dead = Arc::new(AtomicBool::new(false));
        let event_timeout = Duration::from_millis(self.cfg.event_timeout_ms);
        let filter = LogprobFilter {
            default_floor: self.cfg.min_avg_logprob,
            by_language: self.cfg.min_avg_logprob_by_language.clone(),
        };
        let reader = tokio::spawn(read_loop(
            reader_half,
            cmd_rx,
            sink,
            TurnMachine::new(filter),
            event_timeout,
            dead.clone(),
            self.context.clone(),
        ));
        Ok(Box::new(Session {
            writer,
            cmd: cmd_tx,
            reader,
            dead,
            lang,
            event_timeout,
            base_session,
            sent_prompt: base_prompt.clone(),
            base_prompt,
            context: self.context.clone(),
        }))
    }
}

async fn send_event(
    w: &mut SplitSink<WsStream, Message>,
    ev: &ClientEvent,
) -> Result<(), SttError> {
    let json = serde_json::to_string(ev).map_err(|e| SttError::Protocol(e.to_string()))?;
    w.send(Message::Text(json.into()))
        .await
        .map_err(|e| SttError::Other(format!("ws send: {e}")))
}

/// `audio.input.transcription` (GA shape) or `input_audio_transcription` (beta shape).
fn transcription_node(session: &serde_json::Value) -> Option<&serde_json::Value> {
    session
        .pointer("/audio/input/transcription")
        .or_else(|| session.get("input_audio_transcription"))
}

fn transcription_node_mut(session: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if session.pointer("/audio/input/transcription").is_some() {
        session.pointer_mut("/audio/input/transcription")
    } else {
        session.get_mut("input_audio_transcription")
    }
}

/// The base prompt (style + glossary) followed by the recent transcript.
fn context_prompt(base: &str, recent: &str) -> String {
    match (base.trim(), recent.trim()) {
        ("", r) => r.to_string(),
        (b, "") => b.to_string(),
        (b, r) => format!("{b} {r}"),
    }
}

impl Session {
    /// After a commit: the next turn is transcribed with the meeting's latest text as context.
    async fn refresh_prompt(&mut self) -> Result<(), SttError> {
        let Some(mut session) = self.base_session.clone() else {
            return Ok(());
        };
        let prompt = context_prompt(&self.base_prompt, &self.context.get());
        if prompt == self.sent_prompt {
            return Ok(());
        }
        let Some(t) = transcription_node_mut(&mut session) else {
            return Ok(());
        };
        t["prompt"] = serde_json::Value::String(prompt.clone());
        let ev = serde_json::json!({ "type": "session.update", "session": session });
        self.writer
            .send(Message::Text(ev.to_string().into()))
            .await
            .map_err(|e| SttError::Other(format!("ws send: {e}")))?;
        self.sent_prompt = prompt;
        Ok(())
    }

    fn check_alive(&self) -> Result<(), SttError> {
        if self.dead.load(Ordering::Relaxed) || self.reader.is_finished() {
            Err(SttError::Closed)
        } else {
            Ok(())
        }
    }

    async fn append(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        for chunk in pcm_16k.chunks(APPEND_CHUNK_SAMPLES) {
            let audio = encode_pcm16_24k(chunk);
            send_event(&mut self.writer, &ClientEvent::Append { audio }).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl SttSession for Session {
    async fn feed(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        self.check_alive()?;
        self.append(pcm_16k).await
    }

    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError> {
        self.check_alive()?;
        if req.mode == SttMode::ClosedChunk || req.streamed_samples == 0 {
            let audio = req.utterance.audio.clone();
            self.append(&audio).await?;
        }
        let hint = req.utterance.lang.clone().or_else(|| self.lang.clone());
        self.cmd
            .send(ReaderCmd::Commit(hint))
            .map_err(|_| SttError::Closed)?;
        send_event(&mut self.writer, &ClientEvent::Commit).await?;
        self.refresh_prompt().await
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        self.check_alive()?;
        send_event(&mut self.writer, &ClientEvent::Clear).await
    }

    async fn close(mut self: Box<Self>) {
        let _ = self.cmd.send(ReaderCmd::Shutdown);
        let wait = self.event_timeout + Duration::from_secs(1);
        if tokio::time::timeout(wait, &mut self.reader).await.is_err() {
            self.reader.abort();
        }
        let _ = self.writer.close().await;
    }
}

enum Next {
    Event(ServerEvent),
    Ignored,
    Closed,
    TimedOut,
}

async fn next_message(rd: &mut SplitStream<WsStream>, timeout: Option<Duration>) -> Next {
    let fut = rd.next();
    let item = match timeout {
        Some(t) => match tokio::time::timeout(t, fut).await {
            Ok(i) => i,
            Err(_) => return Next::TimedOut,
        },
        None => fut.await,
    };
    match item {
        None | Some(Err(_)) | Some(Ok(Message::Close(_))) => Next::Closed,
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<ServerEvent>(&t) {
            Ok(ev) => Next::Event(ev),
            Err(e) => {
                debug!(error = %e, "unparseable realtime event ignored");
                Next::Ignored
            }
        },
        Some(Ok(_)) => Next::Ignored,
    }
}

async fn read_loop(
    mut rd: SplitStream<WsStream>,
    mut cmd_rx: mpsc::UnboundedReceiver<ReaderCmd>,
    sink: mpsc::Sender<SttEvent>,
    mut turns: TurnMachine,
    event_timeout: Duration,
    dead: Arc<AtomicBool>,
    context: Arc<RecentContext>,
) {
    let mut shutting_down = false;
    let mut cmd_open = true;
    loop {
        let waiting = turns.outstanding() > 0;
        if shutting_down && !waiting {
            break;
        }
        let outs = tokio::select! {
            cmd = cmd_rx.recv(), if cmd_open => {
                match cmd {
                    Some(ReaderCmd::Commit(hint)) => turns.expect_commit(hint),
                    Some(ReaderCmd::Shutdown) => shutting_down = true,
                    None => { cmd_open = false; shutting_down = true; }
                }
                Vec::new()
            }
            msg = next_message(&mut rd, waiting.then_some(event_timeout)) => match msg {
                Next::Event(ev) => turns.on_event(ev),
                Next::Ignored => Vec::new(),
                Next::Closed => { dead.store(true, Ordering::Relaxed); turns.fail_all() }
                Next::TimedOut => {
                    warn!("realtime transcription timed out waiting for events");
                    dead.store(true, Ordering::Relaxed);
                    turns.fail_all()
                }
            },
        };
        let lost = dead.load(Ordering::Relaxed);
        for out in outs {
            let ev = match out {
                Out::Segment {
                    text,
                    lang,
                    confidence,
                    is_early,
                } => {
                    context.push(&text);
                    SttEvent::Segment(SttSegment::new(text, lang, confidence, is_early))
                }
                Out::Partial { text } => SttEvent::Partial { text },
                Out::Done { ok } => SttEvent::UtteranceDone { ok },
            };
            if sink.send(ev).await.is_err() {
                dead.store(true, Ordering::Relaxed);
                return;
            }
        }
        if lost {
            return;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod context_tests {
    use super::*;

    #[test]
    fn context_keeps_the_latest_words_only() {
        let c = RecentContext::default();
        for i in 0..100 {
            c.push(&format!("câu số {i} về app mobile."));
        }
        let t = c.get();
        assert!(t.chars().count() <= CONTEXT_MAX_CHARS);
        assert!(t.ends_with("câu số 99 về app mobile."));
        assert!(t.starts_with("câu"), "cut at a word boundary: {t}");
    }

    #[test]
    fn prompt_is_base_then_recent_text() {
        assert_eq!(context_prompt("Glossary.", ""), "Glossary.");
        assert_eq!(context_prompt("", "mình test"), "mình test");
        assert_eq!(
            context_prompt("Glossary.", "mình test"),
            "Glossary. mình test"
        );
    }

    #[test]
    fn finds_transcription_node_in_both_shapes() {
        let mut ga = serde_json::json!({"audio":{"input":{"transcription":{"prompt":"a"}}}});
        let beta = serde_json::json!({"input_audio_transcription":{"prompt":"b"}});
        assert_eq!(transcription_node(&ga).unwrap()["prompt"], "a");
        assert_eq!(transcription_node(&beta).unwrap()["prompt"], "b");
        transcription_node_mut(&mut ga).unwrap()["prompt"] = "c".into();
        assert_eq!(ga["audio"]["input"]["transcription"]["prompt"], "c");
    }
}
