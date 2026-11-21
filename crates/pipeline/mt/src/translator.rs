//! The [`Translator`] port and its OpenAI-backed implementation.
//!
//! First sentence of a segment: Realtime text session (warm pool, effort minimal, bounded) with
//! a chat-model fallback through [`LlmProvider`] (any provider can plug in, including BYO).
//! Remaining sentences: ONE chat call returning a JSON-schema array ([`Translator::translate_batch`]),
//! falling back to concurrent per-sentence calls (never a sequential loop) when the reply cannot
//! be parsed back into exactly N entries.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::watch;
use tracing::warn;
use warptalk_llm::{JsonSchemaFormat, LlmProvider, Message, Request};

use crate::config::{ChatConfig, MtConfig};
use crate::error::MtError;
use crate::polish;
use crate::prompt::{
    BatchRequest, TranslateRequest, batch_prompt, batch_schema, parse_batch, single_prompt,
};
use crate::realtime::{RealtimeCall, RealtimeClient};

#[derive(Debug, Clone)]
pub struct TranslateOutput {
    pub text: String,
    pub model: String,
    /// First text from the model (Realtime: first delta; chat: completion).
    pub first_token_at: Instant,
    pub done_at: Instant,
}

#[derive(Debug, Clone)]
pub struct BatchOutput {
    /// Exactly `texts.len()` entries, in order.
    pub texts: Vec<String>,
    pub model: String,
    pub done_at: Instant,
}

#[async_trait]
pub trait Translator: Send + Sync {
    /// One sentence. This is the latency path.
    async fn translate(&self, req: &TranslateRequest) -> Result<TranslateOutput, MtError>;
    /// N sentences in one call; the result has exactly N entries or the call is an error.
    async fn translate_batch(&self, req: &BatchRequest) -> Result<BatchOutput, MtError>;
    /// Same-language tidy-up. The stage bounds it and verifies the result; this only asks.
    async fn polish(&self, text: &str, language: &str) -> Result<String, MtError>;
}

pub struct OpenAiTranslator {
    cfg: watch::Receiver<MtConfig>,
    realtime: Option<Arc<RealtimeClient>>,
    llm: Arc<dyn LlmProvider>,
}

impl OpenAiTranslator {
    /// `realtime` is `None` to run on the chat model only (no socket pool).
    pub fn new(
        cfg: watch::Receiver<MtConfig>,
        realtime: Option<Arc<RealtimeClient>>,
        llm: Arc<dyn LlmProvider>,
    ) -> Self {
        Self { cfg, realtime, llm }
    }

    fn snapshot(&self) -> MtConfig {
        self.cfg.borrow().clone()
    }

    fn build_request(
        &self,
        chat: &ChatConfig,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> Request {
        let mut r = Request::new(chat.model.clone(), vec![Message::user(user)]);
        r.system = Some(system);
        r.max_output_tokens = Some(max_tokens);
        // Temperature 0 for determinism (a repeated sentence translates identically and hits
        // the TTS cache), dropped for models that reject it.
        if self.llm.capabilities(&chat.model).accepts_temperature {
            r.temperature = Some(chat.temperature);
        }
        r
    }

    /// Chat call with bounded retries of transient errors (upstream: 2 retries, 0.5 s doubling).
    async fn complete(&self, chat: &ChatConfig, req: Request) -> Result<String, MtError> {
        let mut delay = Duration::from_millis(chat.backoff_ms);
        let mut attempt = 0;
        loop {
            let call = self.llm.complete(&req);
            let res =
                match tokio::time::timeout(Duration::from_millis(chat.request_timeout_ms), call)
                    .await
                {
                    Ok(r) => r.map_err(MtError::from),
                    Err(_) => Err(MtError::Timeout("chat")),
                };
            match res {
                Ok(resp) => {
                    let text = resp.text();
                    let text = text.trim();
                    if text.is_empty() {
                        return Err(MtError::Empty);
                    }
                    return Ok(text.to_string());
                }
                Err(e) => {
                    let retryable = match &e {
                        MtError::Llm(l) => l.is_retryable(),
                        MtError::Timeout(_) => true,
                        _ => false,
                    };
                    if retryable && attempt < chat.retries {
                        attempt += 1;
                        warn!(attempt, error = %e, "chat translation transient error, retrying");
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    async fn chat_single(
        &self,
        cfg: &MtConfig,
        req: &TranslateRequest,
    ) -> Result<TranslateOutput, MtError> {
        let p = single_prompt(req);
        let r = self.build_request(&cfg.chat, p.system, p.user, cfg.chat.max_tokens);
        let text = self.complete(&cfg.chat, r).await?;
        let now = Instant::now();
        Ok(TranslateOutput {
            text,
            model: cfg.chat.model.clone(),
            first_token_at: now,
            done_at: now,
        })
    }
}

#[async_trait]
impl Translator for OpenAiTranslator {
    async fn translate(&self, req: &TranslateRequest) -> Result<TranslateOutput, MtError> {
        let cfg = self.snapshot();
        if cfg.realtime.enabled
            && let Some(rt) = &self.realtime
        {
            let p = single_prompt(req);
            match rt
                .translate(&p.system, &p.user, &RealtimeCall::from(&cfg.realtime))
                .await
            {
                Ok(r) => {
                    return Ok(TranslateOutput {
                        text: r.text,
                        model: cfg.realtime.model.clone(),
                        first_token_at: r.first_token_at,
                        done_at: Instant::now(),
                    });
                }
                // Fall to the chat model: an unbounded Realtime wait would be the cliff, a
                // premature fall-back would be the other one, hence the configured bound.
                Err(e) => warn!(error = %e, "realtime translation failed, using chat fallback"),
            }
        }
        self.chat_single(&cfg, req).await
    }

    async fn translate_batch(&self, req: &BatchRequest) -> Result<BatchOutput, MtError> {
        let cfg = self.snapshot();
        let n = req.texts.len();
        if n == 0 {
            return Ok(BatchOutput {
                texts: Vec::new(),
                model: cfg.chat.model.clone(),
                done_at: Instant::now(),
            });
        }
        let json = self.llm.capabilities(&cfg.chat.model).json_schema;
        let p = batch_prompt(req, json);
        let mut r = self.build_request(
            &cfg.chat,
            p.system,
            p.user,
            cfg.chat.max_tokens.saturating_mul(n as u32),
        );
        if json {
            r.response_format = Some(JsonSchemaFormat {
                name: "translations".into(),
                schema: batch_schema(),
                strict: true,
            });
        }
        let raw = self.complete(&cfg.chat, r).await?;
        if let Some(texts) = parse_batch(&raw, n) {
            return Ok(BatchOutput {
                texts,
                model: cfg.chat.model.clone(),
                done_at: Instant::now(),
            });
        }
        warn!(
            expected = n,
            "batch translation parse mismatch, falling back per sentence"
        );
        let results = futures::future::join_all(req.texts.iter().map(|t| {
            let single = TranslateRequest {
                text: t.clone(),
                source_lang: req.source_lang.clone(),
                target_lang: req.target_lang.clone(),
                glossary: req.glossary.clone(),
                context: req.context.clone(),
                repair: req.repair,
            };
            async move { self.translate(&single).await }
        }))
        .await;
        let mut texts = Vec::with_capacity(n);
        let mut model = cfg.chat.model.clone();
        for r in results {
            let o = r?;
            model = o.model;
            texts.push(o.text);
        }
        Ok(BatchOutput {
            texts,
            model,
            done_at: Instant::now(),
        })
    }

    async fn polish(&self, text: &str, _language: &str) -> Result<String, MtError> {
        let cfg = self.snapshot();
        // Bounded by the input: a formatting pass cannot legitimately need much more room than
        // it was given, and a cap is the cheapest guard against a model that writes an essay.
        let cap = cfg.chat.max_tokens.min(
            u32::try_from(text.len())
                .unwrap_or(u32::MAX)
                .saturating_add(128),
        );
        let r = self.build_request(&cfg.chat, polish::instruction(), text.to_string(), cap);
        self.complete(&cfg.chat, r).await
    }
}
