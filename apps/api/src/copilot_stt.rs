//! Server-side STT for the copilot fallback socket (ADR 0001 item 4, "dispatch"): adapters from
//! the copilot module's `ServerStt` port to `warptalk-stt` (the OpenAI Realtime client) and the
//! workspace glossary. Modules do not depend on pipeline crates, so the wiring lives here.
//!
//! `COPILOT_SERVER_STT`: `off` | `fake` (key-less demos and tests) | `openai` (default when
//! `OPENAI_API_KEY` is set). Model: `COPILOT_STT_MODEL`.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::mpsc;
use warptalk_copilot::application::ports::{
    GlossaryTerms, ServerStt, ServerSttError, ServerSttEvent, ServerSttParams, ServerSttSession,
};
use warptalk_copilot::domain::LanguageMode;
use warptalk_copilot::infrastructure::fake_stt::FakeServerStt;
use warptalk_ingress::EndReason;
use warptalk_kernel::{UserId, WorkspaceId};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, Stamps, SttEvent, SttSession, Utterance,
};
use warptalk_stt::{LangDetectConfig, OpenAiRealtimeStt, OpenAiSttConfig, SessionHints, resolve_language};
use warptalk_transcript::application::glossary_service::GlossaryService;

enum Backend {
    OpenAi(Arc<OpenAiRealtimeStt>),
    /// Any pipeline STT (tests plug `FakeStt`-like implementations in here).
    Generic(Arc<dyn SpeechToText>),
}

/// `ServerStt` over a pipeline `SpeechToText`.
pub struct PipelineServerStt {
    backend: Backend,
    model: String,
    detect: LangDetectConfig,
}

impl PipelineServerStt {
    pub fn openai(cfg: OpenAiSttConfig) -> Self {
        Self {
            model: cfg.model.clone(),
            backend: Backend::OpenAi(Arc::new(OpenAiRealtimeStt::new(cfg))),
            detect: LangDetectConfig::default(),
        }
    }

    pub fn generic(stt: Arc<dyn SpeechToText>, model: impl Into<String>) -> Self {
        Self {
            backend: Backend::Generic(stt),
            model: model.into(),
            detect: LangDetectConfig::default(),
        }
    }
}

/// `COPILOT_SERVER_STT` (see module docs); `None` = server fallback disabled.
pub fn server_stt_from_env() -> Option<Arc<dyn ServerStt>> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mode = get("COPILOT_SERVER_STT").map(|m| m.to_ascii_lowercase());
    match mode.as_deref() {
        Some("off") => None,
        Some("fake") => Some(Arc::new(FakeServerStt::new())),
        Some("openai") | None => {
            let key = get("OPENAI_API_KEY");
            let Some(key) = key else {
                if mode.is_some() {
                    tracing::warn!("COPILOT_SERVER_STT=openai but OPENAI_API_KEY is not set");
                }
                return None;
            };
            let base = get("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com".into());
            let ws = if let Some(r) = base.trim_end_matches('/').strip_prefix("https://") {
                format!("wss://{r}/v1/realtime")
            } else if let Some(r) = base.trim_end_matches('/').strip_prefix("http://") {
                format!("ws://{r}/v1/realtime")
            } else {
                format!("{}/v1/realtime", base.trim_end_matches('/'))
            };
            let mut cfg = OpenAiSttConfig::new(key);
            cfg.base_url = ws;
            if let Some(m) = get("COPILOT_STT_MODEL") {
                cfg.model = m;
            } else {
                cfg.model = "gpt-4o-transcribe".into();
            }
            // Cold-connect per channel: no idle warm sockets for a fallback path.
            cfg.pool_size = 0;
            Some(Arc::new(PipelineServerStt::openai(cfg)))
        }
        Some(other) => {
            tracing::warn!(mode = other, "unknown COPILOT_SERVER_STT; server fallback disabled");
            None
        }
    }
}

struct Session {
    inner: Box<dyn SttSession>,
    room: String,
    speaker: String,
    lang: Option<String>,
    seq: u64,
    fed: usize,
}

#[async_trait]
impl ServerStt for PipelineServerStt {
    async fn open(
        &self,
        params: ServerSttParams,
        events: mpsc::Sender<ServerSttEvent>,
    ) -> Result<Box<dyn ServerSttSession>, ServerSttError> {
        let (tx, mut rx) = mpsc::channel::<SttEvent>(64);
        let sp = SessionParams {
            room: params.session.to_string(),
            speaker: params.channel.as_str().to_owned(),
            lang_hint: params.language.clone(),
        };
        let inner = match &self.backend {
            Backend::OpenAi(stt) => {
                let hints = SessionHints {
                    languages: params.languages.clone(),
                    prompt: params.prompt.clone(),
                };
                stt.open_session_with(sp, tx, &hints).await
            }
            Backend::Generic(stt) => stt.open_session(sp, tx).await,
        }
        .map_err(|e| ServerSttError::Provider(e.to_string()))?;

        let detect = self.detect.clone();
        let (mode, languages) = (params.mode, params.languages.clone());
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let out = match ev {
                    SttEvent::Segment(seg) if !seg.utterance_end && !seg.text.trim().is_empty() => {
                        let language = match mode {
                            LanguageMode::Fixed => Some(seg.lang.clone()),
                            m => Some(resolve_language(
                                &seg.text,
                                Some(&seg.lang),
                                &languages,
                                m == LanguageMode::Auto,
                                &detect,
                            )),
                        };
                        ServerSttEvent::Segment {
                            text: seg.text,
                            language,
                            confidence: seg.confidence,
                            early: seg.is_early,
                        }
                    }
                    SttEvent::Segment(_) => continue,
                    SttEvent::UtteranceDone { ok } => ServerSttEvent::UtteranceDone { ok },
                };
                if events.send(out).await.is_err() {
                    break;
                }
            }
        });
        Ok(Box::new(Session {
            inner,
            room: params.session.to_string(),
            speaker: params.channel.as_str().to_owned(),
            lang: params.language,
            seq: 0,
            fed: 0,
        }))
    }

    fn model(&self) -> String {
        self.model.clone()
    }
}

fn err(e: warptalk_pipeline_runtime::SttError) -> ServerSttError {
    ServerSttError::Provider(e.to_string())
}

#[async_trait]
impl ServerSttSession for Session {
    async fn feed(&mut self, pcm: &[i16]) -> Result<(), ServerSttError> {
        let f: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
        self.inner.feed(&f).await.map_err(err)?;
        self.fed += f.len();
        Ok(())
    }

    async fn commit(&mut self) -> Result<(), ServerSttError> {
        let ms = u32::try_from(self.fed * 1000 / 16_000).unwrap_or(u32::MAX);
        let utt = Utterance {
            room: self.room.clone(),
            speaker: self.speaker.clone(),
            seq: self.seq,
            lang: self.lang.clone(),
            audio: Vec::<f32>::new().into(),
            speech_ms: ms,
            total_ms: ms,
            hangover_ms: 0,
            reason: EndReason::Silence,
            stamps: Stamps::new(Instant::now()),
        };
        let req = CommitRequest {
            utterance: utt,
            mode: warptalk_pipeline_runtime::SttMode::Realtime,
            streamed_samples: self.fed,
        };
        self.seq += 1;
        self.fed = 0;
        self.inner.commit(req).await.map_err(err)
    }

    async fn discard(&mut self) -> Result<(), ServerSttError> {
        self.fed = 0;
        self.inner.discard().await.map_err(err)
    }

    async fn close(self: Box<Self>) {
        self.inner.close().await;
    }
}

/// Copilot's glossary port backed by the transcript module's glossaries: the source terms of
/// every glossary of the workspace the caller belongs to (best effort).
pub struct CopilotGlossary(pub Arc<GlossaryService>);

#[async_trait]
impl GlossaryTerms for CopilotGlossary {
    async fn terms(&self, ws: WorkspaceId, user: UserId, max: usize) -> Vec<String> {
        let Ok(glossaries) = self.0.list_for_workspace(user, ws).await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for g in glossaries {
            let Ok(terms) = self.0.terms(user, g.id).await else {
                continue;
            };
            for t in terms {
                out.push(t.source_term);
                if out.len() >= max {
                    return out;
                }
            }
        }
        out
    }
}
