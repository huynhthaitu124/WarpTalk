//! Prompt assembly, transcript windows and output validation. Pure functions.

use uuid::Uuid;

use super::config::SummaryConfig;
use super::model::{ActionItemDraft, RawOutput, SummaryContent};
use super::ports::{LiveFollowUp, RoomMeta, TranscriptSegment};

const SYSTEM: &str = include_str!("prompts/system.md");
const MAP: &str = include_str!("prompts/map.md");
const REDUCE: &str = include_str!("prompts/reduce.md");
const SINGLE: &str = include_str!("prompts/single.md");

/// Caps on list lengths of one summary.
const MAX_LIST: usize = 60;

pub fn system_prompt(cfg: &SummaryConfig, language: &str) -> String {
    cfg.system_prompt
        .as_deref()
        .unwrap_or(SYSTEM)
        .replace("{{language}}", language)
        .trim()
        .to_owned()
}

pub fn single_instruction(cfg: &SummaryConfig) -> String {
    // A custom map/reduce prompt does not apply to a single pass; the built-in one does.
    let _ = cfg;
    SINGLE.trim().to_owned()
}

pub fn map_instruction(cfg: &SummaryConfig, part: usize, parts: usize) -> String {
    cfg.map_prompt
        .as_deref()
        .unwrap_or(MAP)
        .replace("{{part}}", &part.to_string())
        .replace("{{parts}}", &parts.to_string())
        .trim()
        .to_owned()
}

pub fn reduce_instruction(cfg: &SummaryConfig) -> String {
    cfg.reduce_prompt
        .as_deref()
        .unwrap_or(REDUCE)
        .trim()
        .to_owned()
}

pub fn estimate_tokens(text: &str, chars_per_token: f32) -> u32 {
    crate::suggestion::window::estimate_tokens(text, chars_per_token)
}

/// `[s12] Speaker: text`, one line per segment (reference = 1-based index).
pub fn transcript_lines(segments: &[TranscriptSegment], max_line_chars: usize) -> Vec<String> {
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let text: String = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let text: String = text.chars().take(max_line_chars).collect();
            format!("[s{}] {}: {}", i + 1, s.speaker.trim(), text)
        })
        .collect()
}

/// Greedy windows of consecutive lines, each within `window_tokens` (a line is never split).
pub fn windows(lines: &[String], window_tokens: u32, chars_per_token: f32) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut used = 0u32;
    for l in lines {
        let t = estimate_tokens(l, chars_per_token);
        if !cur.is_empty() && used + t > window_tokens {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(l);
        used += t;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn meta_block(meta: &RoomMeta, followups: &[LiveFollowUp]) -> String {
    let mut s = String::new();
    s.push_str(&format!("Meeting title: {}\n", meta.title.trim()));
    s.push_str(&format!("Spoken language: {}\n", meta.source_language));
    if !meta.target_languages.is_empty() {
        s.push_str(&format!(
            "Translated into: {}\n",
            meta.target_languages.join(", ")
        ));
    }
    if !meta.participants.is_empty() {
        s.push_str(&format!("Participants: {}\n", meta.participants.join(", ")));
    }
    if !followups.is_empty() {
        s.push_str("Follow-ups suggested live during the meeting (hints only, may be wrong):\n");
        for f in followups {
            s.push_str(&format!("- {}", f.task.trim()));
            if let Some(o) = &f.owner {
                s.push_str(&format!(" (owner: {o})"));
            }
            if let Some(d) = &f.due {
                s.push_str(&format!(" (due: {d})"));
            }
            s.push('\n');
        }
    }
    s
}

pub fn transcript_block(lines: &[&str]) -> String {
    format!(
        "Transcript:\n<transcript>\n{}\n</transcript>",
        lines.join("\n")
    )
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Lowercased, whitespace-normalized text speakers and words are looked up in.
pub fn haystack(segments: &[TranscriptSegment]) -> String {
    let mut h = String::new();
    for s in segments {
        h.push_str(&norm(&s.speaker));
        h.push(' ');
        h.push_str(&norm(&s.text));
        h.push('\n');
    }
    h
}

/// Keep `candidate` only when it literally appears in the transcript.
fn literal(candidate: Option<&str>, hay: &str) -> Option<String> {
    let c = candidate?
        .trim()
        .trim_matches(|c: char| c == '.' || c == ',' || c == ';');
    let n = norm(c);
    if n.is_empty() || !hay.contains(&n) {
        return None;
    }
    Some(c.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && seen.insert(s.to_lowercase()))
        .take(MAX_LIST)
        .collect()
}

fn resolve_ref(r: &str, ids: &[Uuid]) -> Option<Uuid> {
    let t = r.trim().trim_matches(|c| c == '[' || c == ']');
    let t = t.strip_prefix(['s', 'S']).unwrap_or(t);
    let n: usize = t.trim().parse().ok()?;
    ids.get(n.checked_sub(1)?).copied()
}

/// Turn a raw model reply into the stored document: owner/due are dropped unless literally in
/// the transcript, unknown segment references are dropped, lists de-duplicated and capped.
pub fn validate(raw: RawOutput, ids: &[Uuid], hay: &str) -> SummaryContent {
    let mut seen = std::collections::HashSet::new();
    let action_items = raw
        .action_items
        .into_iter()
        .filter_map(|a| {
            let task = a.task.trim().to_owned();
            if task.is_empty() || !seen.insert(task.to_lowercase()) {
                return None;
            }
            let mut sources: Vec<Uuid> = Vec::new();
            for r in &a.source_segments {
                if let Some(id) = resolve_ref(r, ids)
                    && !sources.contains(&id)
                {
                    sources.push(id);
                }
            }
            Some(ActionItemDraft {
                task,
                owner: literal(a.owner.as_deref(), hay),
                due: literal(a.due.as_deref(), hay),
                source_segment_ids: sources,
            })
        })
        .take(MAX_LIST)
        .collect();
    SummaryContent {
        summary_short: raw.summary_short.trim().to_owned(),
        summary_detailed: raw.summary_detailed.trim().to_owned(),
        decisions: dedupe(raw.decisions),
        action_items,
        open_questions: dedupe(raw.open_questions),
        topics: dedupe(raw.topics),
        ..Default::default()
    }
}

/// Strip a Markdown code fence some models wrap around JSON.
pub fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::summary::model::RawActionItem;

    fn seg(speaker: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            id: Uuid::now_v7(),
            speaker: speaker.into(),
            text: text.into(),
            language: "en".into(),
            start_ms: 0,
        }
    }

    #[test]
    fn owner_and_due_must_be_literal() {
        let segs = vec![
            seg("Ann", "Bob will send the budget by Friday noon."),
            seg("Bob", "OK."),
        ];
        let ids: Vec<Uuid> = segs.iter().map(|s| s.id).collect();
        let raw = RawOutput {
            action_items: vec![
                RawActionItem {
                    task: "Send the budget".into(),
                    owner: Some("Bob".into()),
                    due: Some("by Friday noon".into()),
                    source_segments: vec!["s1".into(), "S1".into(), "s99".into()],
                },
                RawActionItem {
                    task: "Book room".into(),
                    owner: Some("Zed".into()),
                    due: Some("next Tuesday".into()),
                    source_segments: vec![],
                },
            ],
            ..Default::default()
        };
        let c = validate(raw, &ids, &haystack(&segs));
        assert_eq!(c.action_items[0].owner.as_deref(), Some("Bob"));
        assert_eq!(c.action_items[0].due.as_deref(), Some("by Friday noon"));
        assert_eq!(c.action_items[0].source_segment_ids, vec![ids[0]]);
        assert_eq!(c.action_items[1].owner, None);
        assert_eq!(c.action_items[1].due, None);
    }

    #[test]
    fn windows_split_by_tokens() {
        let lines: Vec<String> = (0..10)
            .map(|i| format!("[s{i}] A: {}", "x".repeat(36)))
            .collect();
        let w = windows(&lines, 30, 4.0);
        assert!(w.len() > 1);
        assert_eq!(w.iter().map(Vec::len).sum::<usize>(), 10);
    }

    #[test]
    fn fence_is_stripped() {
        assert_eq!(strip_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
    }
}
