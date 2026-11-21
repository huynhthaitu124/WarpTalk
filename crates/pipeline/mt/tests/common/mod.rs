#![allow(dead_code, clippy::unwrap_used)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, Usage,
};

type Script = dyn Fn(&Request, usize) -> Result<String, LlmError> + Send + Sync;

/// Fake chat provider: the closure gets the request and the 0-based call index.
pub struct FakeLlm {
    pub calls: AtomicUsize,
    pub requests: Mutex<Vec<Request>>,
    pub json_schema: bool,
    pub delay: Duration,
    script: Box<Script>,
}

impl FakeLlm {
    pub fn new(
        json_schema: bool,
        script: impl Fn(&Request, usize) -> Result<String, LlmError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            json_schema,
            delay: Duration::ZERO,
            script: Box::new(script),
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }
    fn capabilities(&self, _model: &str) -> Capabilities {
        let mut c = Capabilities::conservative();
        c.json_schema = self.json_schema;
        c
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(req.clone());
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let text = (self.script)(req, i)?;
        Ok(Response {
            parts: vec![Part::Text(text)],
            stop_reason: StopReason::End,
            usage: Usage::default(),
            provider_request_id: None,
        })
    }
    async fn stream(
        &self,
        _req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest("not supported by fake".into()))
    }
    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The user text of a request.
pub fn user_text(req: &Request) -> String {
    req.messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
