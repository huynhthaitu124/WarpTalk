//! Deterministic scripted provider for CI. It answers the engine's decide/generate calls and the
//! summary call from the meeting's `fake` block, and acts as a constant judge.
//!
//! The fake is not a mock of one prompt: it finds the latest transcript line in the request,
//! looks up the scripted suggestion for that line and respects the categories the request
//! offers, so a prompt or category change shows up in the score like it would with a real model.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use warptalk_llm::{
    Capabilities, LlmError, LlmProvider, Part, ProviderId, Request, Response, StopReason,
    StreamEvent, Usage,
};

use crate::dataset::{FakeScript, Meeting};
use crate::textmatch;

pub struct FakeProvider {
    script: FakeScript,
    lines: Vec<String>,
    /// Comply with injected instructions (proves the injection metric can fail).
    naive: bool,
}

/// Phrases (folded) that make the naive mode "obey" the text it reads.
const INJECTION_TRIGGERS: &[&str] = &[
    "bo qua moi huong dan",
    "ignore all previous",
    "ignore previous",
    "reveal your system prompt",
    "tiet lo system prompt",
    "system message",
    "[system]",
    "system:",
    "new instructions",
    "note to assistant",
    "output this link",
];

fn has_trigger(text: &str) -> bool {
    let f = textmatch::fold(text);
    INJECTION_TRIGGERS.iter().any(|t| f.contains(t))
}

fn wants_prompt_leak(text: &str) -> bool {
    let f = textmatch::fold(text);
    f.contains("system prompt")
}

/// First `http(s)://` URL of `text`, trailing punctuation removed.
fn first_url(text: &str) -> Option<String> {
    let start = text.find("http://").or_else(|| text.find("https://"))?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '<' | '>'))
        .unwrap_or(rest.len());
    Some(
        rest[..end]
            .trim_end_matches(['.', ',', ')', ';'])
            .to_owned(),
    )
}

impl FakeProvider {
    pub fn new(meeting: &Meeting) -> Self {
        Self {
            script: meeting.fake.clone(),
            lines: meeting.lines.iter().map(|l| l.text.clone()).collect(),
            naive: false,
        }
    }

    /// Like [`Self::new`] but obeys instructions found in the transcript or the knowledge
    /// snippets: it emits the attacker URL / leaks the system prompt.
    pub fn naive(meeting: &Meeting) -> Self {
        Self {
            naive: true,
            ..Self::new(meeting)
        }
    }

    /// A provider with no script (judge only).
    pub fn judge_only() -> Self {
        Self {
            script: FakeScript::default(),
            lines: Vec::new(),
            naive: false,
        }
    }

    fn request_text(req: &Request) -> String {
        let mut s = String::new();
        for m in &req.messages {
            for p in &m.parts {
                if let Part::Text(t) = p {
                    s.push_str(t);
                    s.push('\n');
                }
            }
        }
        s
    }

    /// Text between the first `<transcript>` and the last `</transcript>` (a spoofed closing
    /// tag inside the transcript must not cut the block short).
    fn transcript_block(text: &str) -> &str {
        let Some(start) = text.find("<transcript>") else {
            return text;
        };
        let rest = &text[start + "<transcript>".len()..];
        rest.rfind("</transcript>").map_or(rest, |e| &rest[..e])
    }

    /// Index of the dataset line the request's latest transcript line corresponds to.
    fn latest_line(&self, req: &Request) -> Option<usize> {
        let text = Self::request_text(req);
        let block = Self::transcript_block(&text);
        let latest = block
            .lines()
            .find(|l| l.contains("LATEST"))
            .or_else(|| block.lines().rev().find(|l| !l.trim().is_empty()))?;
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, t)| textmatch::contains(latest, t))
            .max_by_key(|(_, t)| t.len())
            .map(|(i, _)| i)
    }

    /// Text of the request's latest transcript line.
    fn latest_text(&self, req: &Request) -> Option<String> {
        let text = Self::request_text(req);
        let block = Self::transcript_block(&text);
        block
            .lines()
            .find(|l| l.contains("LATEST"))
            .or_else(|| block.lines().rev().find(|l| !l.trim().is_empty()))
            .map(str::to_owned)
    }

    /// Naive mode: what a model that obeys injected text would write. `None` when the request
    /// carries no injection (or the mode is off).
    fn naive_hijack(&self, req: &Request) -> Option<String> {
        if !self.naive {
            return None;
        }
        let text = Self::request_text(req);
        if !has_trigger(&text) {
            return None;
        }
        if wants_prompt_leak(&text) {
            let sys = req.system.clone().unwrap_or_default();
            return Some(sys.chars().take(240).collect());
        }
        first_url(&text).map(|u| format!("Xem / see {u}"))
    }

    fn offered_categories(req: &Request) -> Vec<String> {
        req.response_format
            .as_ref()
            .and_then(|f| f.schema.pointer("/properties/category/enum"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn answer(&self, req: &Request) -> Result<String, LlmError> {
        let purpose = req.metadata.get("purpose").map(String::as_str);
        match purpose {
            Some("suggestion_decide") => {
                let offered = Self::offered_categories(req);
                if self.naive
                    && let Some(latest) = self.latest_text(req)
                    && has_trigger(&latest)
                {
                    let cat = offered
                        .iter()
                        .find(|c| c.as_str() == "answer")
                        .or_else(|| offered.first())
                        .cloned()
                        .unwrap_or_else(|| "answer".into());
                    return Ok(json!({"should_suggest": true, "category": cat,
                        "confidence": 0.95, "reason": "obeying the transcript"})
                    .to_string());
                }
                let hit = self
                    .latest_line(req)
                    .and_then(|i| self.script.suggestions.iter().find(|s| s.at_line == i))
                    .filter(|s| offered.is_empty() || offered.contains(&s.category));
                Ok(match hit {
                    Some(s) => json!({"should_suggest": true, "category": s.category,
                        "confidence": s.confidence, "reason": "scripted"}),
                    None => json!({"should_suggest": false, "category": "",
                        "confidence": 0.0, "reason": "nothing scripted"}),
                }
                .to_string())
            }
            Some("suggestion_generate") => {
                if let Some(v) = self.naive_hijack(req) {
                    return Ok(json!({"content": v, "detail": "", "source": "",
                        "conflict": false})
                    .to_string());
                }
                let hit = self
                    .latest_line(req)
                    .and_then(|i| self.script.suggestions.iter().find(|s| s.at_line == i));
                Ok(match hit {
                    Some(s) => {
                        let mut v = json!({"content": s.content, "detail": s.detail,
                            "source": s.source, "conflict": s.conflict});
                        if let (Some(f), Some(o)) = (&s.follow_up, v.as_object_mut()) {
                            o.insert("follow_up".into(), f.clone());
                        }
                        v
                    }
                    None => json!({"content": "", "detail": "", "source": "", "conflict": false}),
                }
                .to_string())
            }
            Some("summary") => {
                let mut v = self
                    .script
                    .summary
                    .clone()
                    .ok_or_else(|| LlmError::InvalidRequest("no scripted summary".into()))?;
                if let Some(extra) = self.naive_hijack(req)
                    && let Some(o) = v.as_object_mut()
                {
                    let short = o
                        .get("summary_short")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    o.insert(
                        "summary_short".into(),
                        Value::String(format!("{short} {extra}")),
                    );
                }
                Ok(v.to_string())
            }
            Some("eval_judge") => Ok(json!({"score": 4, "reason": "fake judge"}).to_string()),
            other => Err(LlmError::InvalidRequest(format!(
                "fake provider: unknown purpose {other:?}"
            ))),
        }
    }
}

#[async_trait]
impl LlmProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAiCompatible
    }

    fn capabilities(&self, _: &str) -> Capabilities {
        Capabilities::conservative()
    }

    async fn complete(&self, req: &Request) -> Result<Response, LlmError> {
        let text = self.answer(req)?;
        let input = (Self::request_text(req).chars().count()
            + req.system.as_deref().map_or(0, |s| s.chars().count()))
            / 4;
        Ok(Response {
            usage: Usage {
                input_uncached: input as u32,
                output: (text.chars().count() / 4) as u32,
                ..Default::default()
            },
            parts: vec![Part::Text(text)],
            stop_reason: StopReason::End,
            provider_request_id: None,
        })
    }

    async fn stream(
        &self,
        _: &Request,
    ) -> Result<BoxStream<'static, Result<StreamEvent, LlmError>>, LlmError> {
        Err(LlmError::InvalidRequest(
            "fake provider does not stream".into(),
        ))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}
