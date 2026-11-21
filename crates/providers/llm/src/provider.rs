//! Provider traits and capability flags.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::LlmError;
use crate::model::{Embeddings, ProviderId, Request, Response, StreamEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCache {
    None,
    /// Provider caches automatically.
    Implicit,
    /// Caller must place breakpoints (`cache_control`).
    Explicit,
}

/// Static, heuristic per-model capabilities (workspace probes may override them upstream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub tools: bool,
    pub parallel_tools: bool,
    pub forced_tool_choice: bool,
    pub json_schema: bool,
    pub reasoning: bool,
    pub accepts_temperature: bool,
    pub prompt_cache: PromptCache,
    pub streaming_tool_args: bool,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

impl Capabilities {
    /// Text only, nothing assumed.
    pub fn conservative() -> Self {
        Self {
            tools: false,
            parallel_tools: false,
            forced_tool_choice: false,
            json_schema: false,
            reasoning: false,
            accepts_temperature: true,
            prompt_cache: PromptCache::None,
            streaming_tool_args: true,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self, model: &str) -> Capabilities;
    async fn complete(&self, req: &Request) -> Result<Response, LlmError>;
    async fn stream(
        &self,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError>;
    /// Cheap authenticated call (model listing) to check the key and endpoint.
    async fn validate(&self) -> Result<(), LlmError>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Embeddings, LlmError>;
}
