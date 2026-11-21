//! Ports the stage reads on its hot path. Implementations must be cheap (cached): they are
//! awaited in segment order.

use async_trait::async_trait;

use crate::types::SkipReason;

/// Distinct listener languages of a room for a speaker's segment. Listeners wanting the same
/// language share one translation (and one dub). May include the speaker's own language: that
/// yields a same-language passthrough (captions only).
#[async_trait]
pub trait TargetLanguages: Send + Sync {
    async fn targets(&self, room: &str, speaker: &str, source_lang: &str) -> Vec<String>;
}

pub struct StaticTargets(pub Vec<String>);

#[async_trait]
impl TargetLanguages for StaticTargets {
    async fn targets(&self, _room: &str, _speaker: &str, _source_lang: &str) -> Vec<String> {
        self.0.clone()
    }
}

/// Skip rules that depend on room state (spec 13 section 2.5): meeting not started, credits
/// exhausted, room paused. Checked before any model call ("skip before spend", spec 08 item 20).
#[async_trait]
pub trait SkipPolicy: Send + Sync {
    async fn skip_reason(&self, room: &str, speaker: &str) -> Option<SkipReason>;
}

pub struct NeverSkip;

#[async_trait]
impl SkipPolicy for NeverSkip {
    async fn skip_reason(&self, _room: &str, _speaker: &str) -> Option<SkipReason> {
        None
    }
}

/// Static meeting context (title, description), fetched once per room and cached by the stage.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn meeting_context(&self, room: &str) -> Vec<String>;
}

pub struct NoContext;

#[async_trait]
impl ContextProvider for NoContext {
    async fn meeting_context(&self, _room: &str) -> Vec<String> {
        Vec::new()
    }
}
