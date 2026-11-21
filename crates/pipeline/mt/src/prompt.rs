//! Prompt assembly, shared by the Realtime and chat paths so both say the same thing.
//!
//! Ported from upstream `translator.py`. Not ported: the valence marker (a trailing sentiment
//! token for TTS emotion) and the `OUT_OF_MEETING_SCOPE` suppression marker; both change what
//! the model may emit and need their own evaluation before they go on the hot path.

use crate::glossary::{SelectedTerm, render_block};
use crate::lang::language_name;

#[derive(Debug, Clone, PartialEq)]
pub struct TranslateRequest {
    pub text: String,
    pub source_lang: String,
    pub target_lang: String,
    /// Terms already selected for this text (only those that occur in it).
    pub glossary: Vec<SelectedTerm>,
    /// Earlier accepted utterances, context only (never translated). The caller bounds the count
    /// (spec 13 section 2.3: 4).
    pub context: Vec<String>,
    /// Whether to include the ASR-repair clause (evidence-gated by the stage).
    pub repair: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchRequest {
    pub texts: Vec<String>,
    pub source_lang: String,
    pub target_lang: String,
    pub glossary: Vec<SelectedTerm>,
    pub context: Vec<String>,
    pub repair: bool,
}

const SYSTEM: &str = "You are a professional real-time interpreter in a multilingual business \
meeting. Translate the user's message accurately and naturally. Preserve tone, technical terms, \
and speaker intent. Output ONLY the translation - no explanations, no notes, no alternatives.";

const BATCH_SYSTEM_NUMBERED: &str = "You are a professional real-time interpreter in a \
multilingual business meeting. You will receive several numbered sentences, one per line, in the \
form '[n] text'. Translate each sentence accurately and naturally, preserving tone, technical \
terms, and speaker intent. Reply with exactly one line per input sentence, in the same order, \
each formatted as '[n] translation' using the same number n as the input. Output ONLY those \
numbered lines - no explanations, no notes, no alternatives.";

const BATCH_SYSTEM_JSON: &str = "You are a professional real-time interpreter in a multilingual \
business meeting. You will receive several numbered sentences, one per line, in the form \
'[n] text'. Translate each sentence accurately and naturally, preserving tone, technical terms, \
and speaker intent. Reply with a JSON object {\"translations\": [...]} holding exactly one \
translated string per input sentence, in the same order and without the '[n]' prefix.";

/// Only sent on calls that carry something to repair from: a suspected mishearing, meeting
/// context, or a low-confidence transcript. On any other call the instruction can only reach its
/// own fallback (translate literally), which the base prompt already does; measured upstream
/// with no increase in invention across four models.
const ASR_REPAIR: &str = " Your input is automatic speech recognition output, so it may contain \
words that were misheard - most often a name or technical term rendered as similar-sounding \
syllables in the speaker's own language. When the glossary or the meeting context makes it clear \
what was really said, translate what the speaker MEANT. When you have no such evidence, translate \
exactly what is written and invent nothing. Never add, drop, or 'complete' content that is not \
there.";

fn exception_clause(has_glossary: bool) -> String {
    let base = "except for proper nouns/brand names with no natural translation";
    if has_glossary {
        format!("{base}, or terms covered by the glossary below")
    } else {
        base.to_string()
    }
}

fn context_block(context: &[String]) -> String {
    if context.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n\nEarlier accepted utterances from this same meeting are provided only to resolve \
         pronouns, terminology, and topic. Never translate, repeat, or invent content from them:",
    );
    for l in context {
        s.push_str("\n- ");
        s.push_str(l);
    }
    s
}

pub struct Prompt {
    pub system: String,
    pub user: String,
}

pub fn single_prompt(req: &TranslateRequest) -> Prompt {
    let (src, tgt) = (
        language_name(&req.source_lang),
        language_name(&req.target_lang),
    );
    let mut system = SYSTEM.to_string();
    if req.repair {
        system.push_str(ASR_REPAIR);
    }
    let user = format!(
        "Translate from {src} to {tgt}.\nCurrent utterance (translate only this):\n{}\n\n\
         Respond entirely in {tgt} - never leave any word in {src} or switch to a third \
         language, {}.{}{}",
        req.text,
        exception_clause(!req.glossary.is_empty()),
        render_block(&req.glossary),
        context_block(&req.context),
    );
    Prompt { system, user }
}

pub fn batch_prompt(req: &BatchRequest, json: bool) -> Prompt {
    let (src, tgt) = (
        language_name(&req.source_lang),
        language_name(&req.target_lang),
    );
    let mut system = if json {
        BATCH_SYSTEM_JSON
    } else {
        BATCH_SYSTEM_NUMBERED
    }
    .to_string();
    if req.repair {
        system.push_str(ASR_REPAIR);
    }
    let numbered = req
        .texts
        .iter()
        .enumerate()
        .map(|(i, t)| format!("[{}] {t}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!(
        "Translate from {src} to {tgt}:\n{numbered}\n\nRespond entirely in {tgt} - never leave \
         any word in {src} or switch to a third language, {}.{}{}",
        exception_clause(!req.glossary.is_empty()),
        render_block(&req.glossary),
        context_block(&req.context),
    );
    Prompt { system, user }
}

/// JSON schema of the batch reply.
pub fn batch_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"translations": {"type": "array", "items": {"type": "string"}}},
        "required": ["translations"],
        "additionalProperties": false
    })
}

/// Parses a batch reply into exactly `n` strings, or `None` on any mismatch (the caller then
/// falls back to concurrent per-sentence calls, so a charge or a dub is never attached to the
/// wrong sentence). Accepts `{"translations": [...]}`, a bare array (optionally in a code
/// fence), or numbered `[n] text` lines.
pub fn parse_batch(raw: &str, n: usize) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(unfenced) {
        let arr = match &v {
            serde_json::Value::Array(a) => Some(a),
            serde_json::Value::Object(o) => o.get("translations").and_then(|t| t.as_array()),
            _ => None,
        };
        if let Some(a) = arr {
            let strings: Option<Vec<String>> = a
                .iter()
                .map(|x| x.as_str().map(|s| s.trim().to_string()))
                .collect();
            return strings.filter(|s| s.len() == n && s.iter().all(|t| !t.is_empty()));
        }
    }
    let mut parsed: Vec<Option<String>> = vec![None; n];
    for line in trimmed.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((idx, text)) = rest.split_once(']') else {
            continue;
        };
        if let Ok(i) = idx.trim().parse::<usize>()
            && (1..=n).contains(&i)
        {
            parsed[i - 1] = Some(text.trim().to_string());
        }
    }
    parsed
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .filter(|v| v.iter().all(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::{GlossaryTerm, MatchKind};

    fn req() -> TranslateRequest {
        TranslateRequest {
            text: "Deploy to staging.".into(),
            source_lang: "en".into(),
            target_lang: "vi-VN".into(),
            glossary: vec![],
            context: vec![],
            repair: false,
        }
    }

    #[test]
    fn single_prompt_is_lean_without_evidence() {
        let p = single_prompt(&req());
        assert!(!p.system.contains("speech recognition"));
        assert!(!p.user.contains("glossary"));
        assert!(!p.user.contains("Earlier accepted"));
        assert!(p.user.contains("from English to Vietnamese"));
    }

    #[test]
    fn repair_clause_glossary_and_context_appear_when_given() {
        let mut r = req();
        r.repair = true;
        r.glossary = vec![SelectedTerm {
            term: GlossaryTerm::new("staging", "moi truong staging"),
            kind: MatchKind::Exact,
        }];
        r.context = (3..=6).map(|i| format!("line {i}")).collect();
        let p = single_prompt(&r);
        assert!(p.system.contains("speech recognition"));
        assert!(p.user.contains("terms covered by the glossary below"));
        assert!(p.user.contains("moi truong staging"));
        assert!(p.user.contains("line 6") && p.user.contains("line 3"));
    }

    #[test]
    fn parses_json_array_fenced_and_numbered() {
        assert_eq!(
            parse_batch(r#"{"translations":["a","b"]}"#, 2).unwrap(),
            ["a", "b"]
        );
        assert_eq!(
            parse_batch("```json\n[\"a\",\"b\"]\n```", 2).unwrap(),
            ["a", "b"]
        );
        assert_eq!(parse_batch("[2] bee\n[1] ay", 2).unwrap(), ["ay", "bee"]);
        assert!(parse_batch(r#"{"translations":["a"]}"#, 2).is_none());
        assert!(parse_batch("[1] only one", 2).is_none());
        assert!(parse_batch(r#"{"translations":["a",""]}"#, 2).is_none());
        assert!(parse_batch("garbage", 1).is_none());
    }
}
