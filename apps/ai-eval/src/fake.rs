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
}

impl FakeProvider {
    pub fn new(meeting: &Meeting) -> Self {
        Self {
            script: meeting.fake.clone(),
            lines: meeting.lines.iter().map(|l| l.text.clone()).collect(),
        }
    }

    /// A provider with no script (judge only).
    pub fn judge_only() -> Self {
        Self {
            script: FakeScript::default(),
            lines: Vec::new(),
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

    /// Index of the dataset line the request's latest transcript line corresponds to.
    fn latest_line(&self, req: &Request) -> Option<usize> {
        let text = Self::request_text(req);
        let block = text
            .split("<transcript>")
            .nth(1)
            .and_then(|b| b.split("</transcript>").next())
            .unwrap_or(&text);
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
                let hit = self
                    .latest_line(req)
                    .and_then(|i| self.script.suggestions.iter().find(|s| s.at_line == i))
                    .filter(|s| offered.is_empty() || offered.iter().any(|c| *c == s.category));
                Ok(match hit {
                    Some(s) => json!({"should_suggest": true, "category": s.category,
                        "confidence": s.confidence, "reason": "scripted"}),
                    None => json!({"should_suggest": false, "category": "",
                        "confidence": 0.0, "reason": "nothing scripted"}),
                }
                .to_string())
            }
            Some("suggestion_generate") => {
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
            Some("summary") => self
                .script
                .summary
                .as_ref()
                .map(Value::to_string)
                .ok_or_else(|| LlmError::InvalidRequest("no scripted summary".into())),
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
        Err(LlmError::InvalidRequest("fake provider does not stream".into()))
    }

    async fn validate(&self) -> Result<(), LlmError> {
        Ok(())
    }
}
