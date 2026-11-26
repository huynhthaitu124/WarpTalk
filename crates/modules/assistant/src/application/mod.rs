//! Use cases (commands/queries) and ports (traits) implemented elsewhere.

pub mod chat;
pub mod chat_ports;
pub mod llm_config;
pub mod ports;

pub use chat::{ChatService, ChatServiceDeps, PageContext, SendAck, SendCommand};
pub use llm_config::{
    ConfigView, LlmConfigService, PutConfigCommand, PutConfigResult, TestOutcome,
};
