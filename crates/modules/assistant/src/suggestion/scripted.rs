//! Scripted suggestion provider for local review without an AI key (dev test mode only).
//!
//! It stands in for the LLM behind [`LlmResolver`]; everything else (gates, cooldown, budgets,
//! hygiene, dedup, publish, SSE) is the real engine. The script is a JSON file re-read on every
//! call so it can be edited while a meeting is running:
//!
//! ```json
//! { "entries": [
//!   { "match": ["budget", "ngân sách"], "category": "consideration", "confidence": 0.9,
//!     "content": "...", "detail": "...", "source": "",
//!     "translations": { "vi": { "content": "...", "detail": "..." } },
//!     "follow_up": { "owner": "Lan", "task": "...", "due": "Friday" } }
//! ] }
//! ```
//!
//! The decide call matches the `<-- LATEST` transcript line (case-insensitive substring of any
//! `match` keyword); the first matching entry wins. No match means "do not suggest".

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use warptalk_kernel::WorkspaceId;
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, Usage,
};

use super::ports::{DataBoundary, LlmResolver, Purpose, ResolveError, ResolvedLlm};

const LATEST_MARK: &str = "<-- LATEST";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ScriptEntry {
    #[serde(rename = "match")]
    pub keywords: Vec<String>,
    pub category: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    pub content: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub translations: Map<String, Value>,
    #[serde(default)]
    pub follow_up: Option<Value>,
}

fn default_confidence() -> f64 {
    0.9
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Script {
    #[serde(default)]
    pub entries: Vec<ScriptEntry>,
}

impl Script {
    pub fn parse(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// First entry whose keyword occurs in `line` (case-insensitive).
    pub fn find(&self, line: &str) -> Option<&ScriptEntry> {
        let line = line.to_lowercase();
        self.entries.iter().find(|e| {
            e.keywords
                .iter()
                .map(|k| k.trim().to_lowercase())
                .any(|k| !k.is_empty() && line.contains(&k))
        })
    }
}

/// The transcript line the engine marks as the newest one.
pub fn latest_line(prompt: &str) -> Option<&str> {
    prompt
        .lines()
        .find(|l| l.ends_with(LATEST_MARK))
        .map(|l| l.trim_end_matches(LATEST_MARK).trim_end())
}

/// Canned decide / generate answers in the engine's JSON contract.
pub fn answer(script: &Script, req: &Request) -> Value {
    let user: String = req
        .messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let entry = latest_line(&user).and_then(|l| script.find(l));
    let purpose = req.metadata.get("purpose").map(String::as_str);
    if purpose == Some(Purpose::SuggestionGenerate.as_str()) {
        let Some(e) = entry else {
            return json!({"content": "", "detail": "", "source": "", "conflict": false});
        };
        let mut v = json!({
            "content": e.content,
            "detail": e.detail,
            "source": e.source,
            "conflict": false,
        });
        if let Some(o) = v.as_object_mut() {
            if !e.translations.is_empty() {
                o.insert("translations".into(), Value::Object(e.translations.clone()));
            }
            if let Some(f) = &e.follow_up {
                o.insert("follow_up".into(), f.clone());
            }
        }
        return v;
    }
    match entry {
        Some(e) => json!({
            "should_suggest": true,
            "category": e.category,
            "confidence": e.confidence,
            "reason": "test script match",
        }),
        None => json!({
            "should_suggest": false,
            "category": "",
            "confidence": 0.0,
            "reason": "no test script match",
        }),
    }
}

/// [`LlmProvider`] that answers from the script file.
pub struct ScriptedProvider {
    path: PathBuf,
}

impl ScriptedProvider {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    async fn load(&self) -> Result<Script, LlmError> {
        let raw = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|e| LlmError::InvalidRequest(format!("test script unreadable: {e}")))?;
        Script::parse(&raw)
            .map_err(|e| LlmError::InvalidRequest(format!("test script is not valid JSON: {e}")))
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAiCompatible
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities::conservative()
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let script = match self.load().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "suggestion test script");
                return Err(e);
            }
        };
        Ok(Response {
            parts: vec![Part::Text(answer(&script, req).to_string())],
            stop_reason: StopReason::End,
            usage: Usage::default(),
            provider_request_id: None,
        })
    }

    async fn stream(
        &self,
        _req: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest(
            "streaming is not supported by the test script provider".into(),
        ))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        self.load().await.map(|_| ())
    }
}

/// Resolver that hands every suggestion purpose to the scripted provider. The data never leaves
/// the process, so it is workspace-controlled (no external-LLM consent gate).
pub struct ScriptedResolver {
    provider: Arc<ScriptedProvider>,
}

impl ScriptedResolver {
    pub fn new(path: PathBuf) -> Self {
        Self {
            provider: Arc::new(ScriptedProvider::new(path)),
        }
    }
}

#[async_trait]
impl LlmResolver for ScriptedResolver {
    async fn for_purpose(
        &self,
        _workspace: WorkspaceId,
        _purpose: Purpose,
    ) -> Result<ResolvedLlm, ResolveError> {
        Ok(ResolvedLlm {
            provider: self.provider.clone(),
            model: Some("test-script".into()),
            boundary: DataBoundary::WorkspaceControlled,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use warptalk_llm::Message;

    fn script() -> Script {
        Script::parse(
            r#"{"entries":[
              {"match":["budget"],"category":"consideration","content":"Check Q3 spend",
               "detail":"d","translations":{"vi":{"content":"Xem chi Q3","detail":"x"}}},
              {"match":["Lan will"],"category":"follow_up","content":"Lan sends deck",
               "follow_up":{"owner":"Lan","task":"send deck","due":null}}
            ]}"#,
        )
        .unwrap()
    }

    fn req(purpose: Purpose, transcript: &str) -> Request {
        let mut r = Request::new("m", vec![Message::user(transcript.to_string())]);
        r.metadata.insert("purpose".into(), purpose.as_str().into());
        r
    }

    #[test]
    fn decide_matches_only_the_latest_line() {
        let t = "<transcript>\n[a] budget talk\n[b] hello there   <-- LATEST\n</transcript>";
        let v = answer(&script(), &req(Purpose::SuggestionDecide, t));
        assert_eq!(v["should_suggest"], false);
        let t = "<transcript>\n[a] hi\n[b] What about the BUDGET?   <-- LATEST\n</transcript>";
        let v = answer(&script(), &req(Purpose::SuggestionDecide, t));
        assert_eq!(v["should_suggest"], true);
        assert_eq!(v["category"], "consideration");
    }

    #[test]
    fn generate_returns_content_translations_and_follow_up() {
        let t = "[#s1 b] the budget   <-- LATEST";
        let v = answer(&script(), &req(Purpose::SuggestionGenerate, t));
        assert_eq!(v["content"], "Check Q3 spend");
        assert_eq!(v["translations"]["vi"]["content"], "Xem chi Q3");
        let t = "[#s2 b] Lan will send it   <-- LATEST";
        let v = answer(&script(), &req(Purpose::SuggestionGenerate, t));
        assert_eq!(v["follow_up"]["owner"], "Lan");
    }

    #[test]
    fn latest_line_strips_marker() {
        assert_eq!(latest_line("x\n[a] hi   <-- LATEST\ny"), Some("[a] hi"));
        assert_eq!(latest_line("nothing"), None);
    }
}
