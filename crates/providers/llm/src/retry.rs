//! Decorators: bounded retry with jittered backoff, and per-call timeout.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::LlmError;
use crate::model::{ProviderId, Request, Response, StreamEvent};
use crate::provider::{Capabilities, LlmProvider};

/// Retries only retryable errors, honoring `retry_after`, within a total deadline.
/// Streams are retried only while *establishing* the stream, never mid-stream.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Give up rather than sleep past this much total elapsed time.
    pub total_deadline: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(8),
            total_deadline: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    pub fn wrap(self, inner: Arc<dyn LlmProvider>) -> Arc<dyn LlmProvider> {
        Arc::new(Retrying {
            inner,
            policy: self,
        })
    }

    fn delay(&self, attempt: u32, err: &LlmError) -> Duration {
        let exp = self
            .base_delay
            .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
            .min(self.max_delay);
        // Equal jitter: between 50% and 100% of the exponential step.
        let frac = (uuid::Uuid::new_v4().as_u128() % 1000) as f64 / 1000.0;
        let jittered = exp.mul_f64(0.5 + 0.5 * frac);
        match err.retry_after() {
            Some(ra) => jittered.max(ra),
            None => jittered,
        }
    }

    async fn run<T, F, Fut>(&self, mut op: F) -> Result<T, LlmError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, LlmError>>,
    {
        let start = Instant::now();
        let mut attempt = 0u32;
        loop {
            match op().await {
                Ok(v) => return Ok(v),
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    let delay = self.delay(attempt, &e);
                    if start.elapsed().saturating_add(delay) > self.total_deadline {
                        return Err(e);
                    }
                    tracing::debug!(attempt, ?delay, "retrying LLM call");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

struct Retrying {
    inner: Arc<dyn LlmProvider>,
    policy: RetryPolicy,
}

#[async_trait]
impl LlmProvider for Retrying {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let inner = &self.inner;
        self.policy.run(|| inner.complete(req)).await
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        let inner = &self.inner;
        self.policy.run(|| inner.stream(req)).await
    }
    async fn validate(&self) -> Result<(), LlmError> {
        let inner = &self.inner;
        self.policy.run(|| inner.validate()).await
    }
}

/// Per-call timeout. For streams it bounds stream establishment and the gap between events.
pub struct Timeout;

impl Timeout {
    pub fn wrap(limit: Duration, inner: Arc<dyn LlmProvider>) -> Arc<dyn LlmProvider> {
        Arc::new(WithTimeout { inner, limit })
    }
}

struct WithTimeout {
    inner: Arc<dyn LlmProvider>,
    limit: Duration,
}

#[async_trait]
impl LlmProvider for WithTimeout {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }
    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        tokio::time::timeout(self.limit, self.inner.complete(req))
            .await
            .map_err(|_| LlmError::Timeout)?
    }
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        let src = tokio::time::timeout(self.limit, self.inner.stream(req))
            .await
            .map_err(|_| LlmError::Timeout)??;
        let limit = self.limit;
        let out = futures::stream::unfold(Some(src), move |state| async move {
            let mut src = state?;
            match tokio::time::timeout(limit, src.next()).await {
                Ok(Some(item)) => Some((item, Some(src))),
                Ok(None) => None,
                Err(_) => Some((Err(LlmError::Timeout), None)),
            }
        });
        Ok(out.boxed())
    }
    async fn validate(&self) -> Result<(), LlmError> {
        tokio::time::timeout(self.limit, self.inner.validate())
            .await
            .map_err(|_| LlmError::Timeout)?
    }
}
