//! [`OpenAiRealtimeStt`]: the `SpeechToText` implementation over the Realtime WebSocket.
//!
//! One claimed socket = one speaker session. Two halves:
//! - the **writer** (inside [`SttSession`] methods): `feed` appends base64 pcm16 24 kHz,
//!   `commit` issues `input_audio_buffer.commit` and returns immediately;
//! - the **reader task**: parses server events into [`TurnMachine`] and forwards its output
//!   (early sentences, remainder, `UtteranceDone`) to the runtime sink.
//!
//! The reader is told about each commit (over an unbounded channel, *before* the commit is
//! written) so `committed` events can be matched to commits in order.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};
use warptalk_audio::{SAMPLE_RATE_16K, f32_to_i16, resample};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttError, SttEvent, SttMode, SttSegment, SttSession,
};

use crate::config::{OpenAiSttConfig, base_language};
use crate::pool::{WarmPool, WsStream};
use crate::protocol::{
    ClientEvent, REALTIME_SAMPLE_RATE, ServerEvent, SessionConfig, TranscriptionConfig,
};
use crate::turns::{Out, TurnMachine};

/// Samples (16 kHz) per `append` message: 1 s, a multiple of the 3:2 resampling ratio.
const APPEND_CHUNK_SAMPLES: usize = 16_000;

pub struct OpenAiRealtimeStt {
    cfg: Arc<OpenAiSttConfig>,
    pool: Arc<WarmPool>,
}

impl OpenAiRealtimeStt {
    pub fn new(cfg: OpenAiSttConfig) -> Self {
        let cfg = Arc::new(cfg);
        let pool = WarmPool::new(cfg.clone());
        Self { cfg, pool }
    }

    /// Fills the pool; call once at startup so the first speaker finds warm sockets.
    pub async fn warm_up(&self) -> usize {
        self.pool.warm_up().await
    }

    pub fn pool(&self) -> &Arc<WarmPool> {
        &self.pool
    }

    pub fn config(&self) -> &OpenAiSttConfig {
        &self.cfg
    }

    /// The `session.update` for a speaker.
    pub fn session_config(&self, lang: Option<&str>) -> SessionConfig {
        self.session_config_with(lang, &SessionHints::default())
    }

    /// Like [`Self::session_config`] with extra hints: more expected languages (plural-hint
    /// models only; single-hint models get the primary) and a vocabulary prompt.
    pub fn session_config_with(&self, lang: Option<&str>, hints: &SessionHints) -> SessionConfig {
        // The realtime API takes ISO-639-1 codes only ("en", not "en-US"): a region tag is
        // rejected with `invalid_value` and the session then transcribes nothing.
        let lang = lang
            .or(self.cfg.language_hint.as_deref())
            .map(base_language);
        let lang = lang.as_deref();
        let extras: Vec<String> = hints.languages.iter().map(|l| base_language(l)).collect();
        let mut t = TranscriptionConfig {
            model: self.cfg.model.clone(),
            prompt: hints.prompt.clone().filter(|p| !p.trim().is_empty()),
            ..TranscriptionConfig::default()
        };
        if let Some(l) = lang.filter(|l| *l != "auto") {
            if self.cfg.uses_plural_languages() {
                // Declared language first; English stays expected so embedded English product
                // terms are not forced into a phonetic rendering (upstream finding).
                t.languages.push(l.to_string());
                for extra in &extras {
                    if extra != l && !t.languages.contains(extra) {
                        t.languages.push(extra.clone());
                    }
                }
                if l != "en" && !t.languages.iter().any(|x| x == "en") {
                    t.languages.push("en".into());
                }
            } else {
                t.language = Some(l.to_string());
            }
        } else if self.cfg.uses_plural_languages() && !extras.is_empty() {
            for extra in extras {
                if !t.languages.contains(&extra) {
                    t.languages.push(extra);
                }
            }
        }
        SessionConfig::transcription(t, self.cfg.wants_logprobs())
    }

    /// `open_session` with [`SessionHints`] (the plain trait method uses the defaults).
    pub async fn open_session_with(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
        hints: &SessionHints,
    ) -> Result<Box<dyn SttSession>, SttError> {
        let socket = self.pool.claim().await?;
        let (mut writer, reader_half) = socket.ws.split();
        let lang = params.lang_hint.or_else(|| self.cfg.language_hint.clone());
        // Fire-and-forget: the server applies events in order, so audio may follow directly.
        send_event(
            &mut writer,
            &ClientEvent::SessionUpdate {
                session: self.session_config_with(lang.as_deref(), hints),
            },
        )
        .await?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let dead = Arc::new(AtomicBool::new(false));
        let reader = tokio::spawn(read_loop(
            reader_half,
            cmd_rx,
            sink,
            TurnMachine::new(self.cfg.logprob_filter()),
            self.cfg.event_timeout(),
            dead.clone(),
        ));
        Ok(Box::new(OpenAiSession {
            writer,
            cmd: cmd_tx,
            reader,
            dead,
            lang,
            event_timeout: self.cfg.event_timeout(),
        }))
    }
}

/// Optional per-session hints beyond the single language of `SessionParams`.
#[derive(Debug, Clone, Default)]
pub struct SessionHints {
    /// Other languages to expect (code-switching), base codes.
    pub languages: Vec<String>,
    /// Vocabulary / glossary prompt.
    pub prompt: Option<String>,
}

enum ReaderCmd {
    Commit(Option<String>),
    /// Stop once every outstanding commit has finished.
    Shutdown,
}

struct OpenAiSession {
    writer: SplitSink<WsStream, Message>,
    cmd: mpsc::UnboundedSender<ReaderCmd>,
    reader: JoinHandle<()>,
    dead: Arc<AtomicBool>,
    lang: Option<String>,
    event_timeout: Duration,
}

#[async_trait]
impl SpeechToText for OpenAiRealtimeStt {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        self.open_session_with(params, sink, &SessionHints::default())
            .await
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

/// 16 kHz f32 -> base64 of little-endian pcm16 at 24 kHz.
pub fn encode_pcm16_24k(pcm_16k: &[f32]) -> String {
    let up = resample(pcm_16k, SAMPLE_RATE_16K, REALTIME_SAMPLE_RATE);
    let mut bytes = Vec::with_capacity(up.len() * 2);
    for s in f32_to_i16(&up) {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    B64.encode(bytes)
}

impl OpenAiSession {
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
impl SttSession for OpenAiSession {
    async fn feed(&mut self, pcm_16k: &[f32]) -> Result<(), SttError> {
        self.check_alive()?;
        self.append(pcm_16k).await
    }

    async fn commit(&mut self, req: CommitRequest) -> Result<(), SttError> {
        self.check_alive()?;
        // Nothing was streamed (closed-chunk mode, or live streaming failed): send it all now.
        if req.mode == SttMode::ClosedChunk || req.streamed_samples == 0 {
            let audio = req.utterance.audio.clone();
            self.append(&audio).await?;
        }
        let hint = req.utterance.lang.clone().or_else(|| self.lang.clone());
        self.cmd
            .send(ReaderCmd::Commit(hint))
            .map_err(|_| SttError::Closed)?;
        send_event(&mut self.writer, &ClientEvent::Commit).await
    }

    async fn discard(&mut self) -> Result<(), SttError> {
        self.check_alive()?;
        send_event(&mut self.writer, &ClientEvent::Clear).await
    }

    async fn close(mut self: Box<Self>) {
        let _ = self.cmd.send(ReaderCmd::Shutdown);
        // Grace: the reader's own per-event timeout bounds a stuck commit.
        let wait = self.event_timeout + Duration::from_secs(1);
        if tokio::time::timeout(wait, &mut self.reader).await.is_err() {
            self.reader.abort();
        }
        let _ = self.writer.close().await;
    }
}

async fn read_loop(
    mut rd: SplitStream<WsStream>,
    mut cmd_rx: mpsc::UnboundedReceiver<ReaderCmd>,
    sink: mpsc::Sender<SttEvent>,
    mut turns: TurnMachine,
    event_timeout: Duration,
    dead: Arc<AtomicBool>,
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
                } => SttEvent::Segment(SttSegment::new(text, lang, confidence, is_early)),
                Out::Done { ok } => SttEvent::UtteranceDone { ok },
            };
            if sink.send(ev).await.is_err() {
                debug!("stt sink closed; stopping reader");
                dead.store(true, Ordering::Relaxed);
                return;
            }
        }
        if lost {
            return;
        }
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
