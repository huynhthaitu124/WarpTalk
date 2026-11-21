//! The ONE typed schema of a vector point's payload, used by the indexer (writer) and the
//! search path (reader). Upstream wrote snake_case keys under `payload` while the reader looked
//! for camelCase top-level keys, so citations came back empty; here both sides go through
//! [`ChunkPayload::to_value`] / [`ChunkPayload::from_value`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::facts::FactCategory;

/// Payload field names used in filters (must match the serde names of [`ChunkPayload`]).
pub mod fields {
    pub const WORKSPACE_ID: &str = "workspace_id";
    pub const SOURCE_TYPE: &str = "source_type";
    pub const SOURCE_ID: &str = "source_id";
    pub const MEETING_ID: &str = "meeting_id";
    pub const KIND: &str = "kind";
    pub const PAGE: &str = "page";
    pub const AI_RETRIEVAL: &str = "ai_retrieval";
    pub const RETENTION_STATE: &str = "retention_state";
    pub const DELETION_STATE: &str = "deletion_state";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Document,
    Transcript,
    MeetingSummary,
    GlossaryTerm,
    GlobalGlossaryTerm,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Transcript => "transcript",
            Self::MeetingSummary => "meeting_summary",
            Self::GlossaryTerm => "glossary_term",
            Self::GlobalGlossaryTerm => "global_glossary_term",
        }
    }
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "document" => Ok(Self::Document),
            "transcript" => Ok(Self::Transcript),
            "meeting_summary" => Ok(Self::MeetingSummary),
            "glossary_term" => Ok(Self::GlossaryTerm),
            "global_glossary_term" => Ok(Self::GlobalGlossaryTerm),
            other => Err(format!("unknown source_type `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    #[default]
    Content,
    Fact,
}

impl ChunkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Fact => "fact",
        }
    }
}

fn default_active() -> String {
    "active".to_owned()
}

fn default_true() -> bool {
    true
}

/// Everything stored with a vector. Optional fields are omitted when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkPayload {
    pub workspace_id: Uuid,
    pub chunk_id: Uuid,
    pub source_type: SourceType,
    /// Document id, transcript id, room id (meeting summary) or term id.
    pub source_id: String,
    #[serde(default)]
    pub kind: ChunkKind,
    pub ordinal: u32,
    pub text: String,
    /// Document title or meeting title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 1-based page (PDFs), from the extractor's page markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heading_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segment_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<String>,
    /// Meeting (translation room) the chunk belongs to: transcripts and meeting summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default = "default_true")]
    pub ai_retrieval: bool,
    #[serde(default = "default_active")]
    pub retention_state: String,
    #[serde(default = "default_active")]
    pub deletion_state: String,
    pub indexed_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_category: Option<FactCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// Content chunks a fact was derived from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_chunk_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid chunk payload: {0}")]
pub struct PayloadError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    Document,
    Transcript,
    Meeting,
    Glossary,
    Knowledge,
}

/// What the assistant needs to cite a chunk: document title + page, or meeting + timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub kind: CitationKind,
    pub title: String,
    /// Document id or meeting id: what the client opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub heading_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub segment_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// Ready-to-show text, e.g. `Q3 Plan, p. 4` or `Weekly sync @ 12:34`.
    pub label: String,
}

/// `m:ss`, or `h:mm:ss` from one hour.
pub fn format_timestamp(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl ChunkPayload {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_value(v: &Value) -> Result<Self, PayloadError> {
        serde_json::from_value(v.clone()).map_err(|e| PayloadError(e.to_string()))
    }

    pub fn citation(&self) -> Citation {
        let from_meeting = matches!(
            self.source_type,
            SourceType::Transcript | SourceType::MeetingSummary
        );
        let is_fact = self.kind == ChunkKind::Fact;
        let mut c = Citation {
            kind: CitationKind::Document,
            title: String::new(),
            ref_id: None,
            page: None,
            heading_path: Vec::new(),
            start_ms: None,
            end_ms: None,
            segment_ids: Vec::new(),
            speaker: None,
            label: String::new(),
        };
        match self.source_type {
            SourceType::GlossaryTerm | SourceType::GlobalGlossaryTerm => {
                c.kind = CitationKind::Glossary;
                c.title = self.title.clone().unwrap_or_else(|| "Glossary".to_owned());
                c.label = c.title.clone();
            }
            _ if from_meeting => {
                c.kind = match (self.source_type, is_fact) {
                    (_, true) => CitationKind::Knowledge,
                    (SourceType::Transcript, false) => CitationKind::Transcript,
                    _ => CitationKind::Meeting,
                };
                c.title = self.title.clone().unwrap_or_else(|| "Meeting".to_owned());
                c.ref_id = self
                    .meeting_id
                    .clone()
                    .or_else(|| Some(self.source_id.clone()));
                c.start_ms = self.start_ms;
                c.end_ms = self.end_ms;
                c.segment_ids = self.segment_ids.clone();
                c.speaker = (self.speakers.len() == 1).then(|| self.speakers[0].clone());
                c.label = match self.start_ms {
                    Some(ms) => format!("{} @ {}", c.title, format_timestamp(ms)),
                    None if self.source_type == SourceType::MeetingSummary => {
                        format!("{} (summary)", c.title)
                    }
                    None => c.title.clone(),
                };
            }
            _ => {
                c.kind = if is_fact {
                    CitationKind::Knowledge
                } else {
                    CitationKind::Document
                };
                c.title = self
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled document".to_owned());
                c.ref_id = Some(self.source_id.clone());
                c.page = self.page;
                c.heading_path = self.heading_path.clone();
                c.label = match (self.page, self.heading_path.last()) {
                    (Some(p), _) => format!("{}, p. {p}", c.title),
                    (None, Some(h)) => format!("{} > {h}", c.title),
                    _ => c.title.clone(),
                };
            }
        }
        c
    }
}

/// Canonical form of an id: hyphenated lowercase when it parses as a UUID (producers send both
/// the `N` and hyphenated forms), otherwise unchanged.
pub fn normalize_id(id: &str) -> String {
    Uuid::parse_str(id.trim())
        .map(|u| u.to_string())
        .unwrap_or_else(|_| id.trim().to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> ChunkPayload {
        ChunkPayload {
            workspace_id: Uuid::now_v7(),
            chunk_id: Uuid::now_v7(),
            source_type: SourceType::Document,
            source_id: Uuid::now_v7().to_string(),
            kind: ChunkKind::Content,
            ordinal: 3,
            text: "hello".into(),
            title: Some("Q3 Plan".into()),
            page: Some(4),
            heading_path: vec!["Budget".into(), "Hiring".into()],
            start_ms: None,
            end_ms: None,
            segment_ids: vec![],
            speakers: vec![],
            meeting_id: None,
            revision: Some("2".into()),
            ai_retrieval: true,
            retention_state: "active".into(),
            deletion_state: "active".into(),
            indexed_at_ms: 1_700_000_000_000,
            fact: None,
            fact_category: None,
            confidence: None,
            source_chunk_ids: vec![],
        }
    }

    #[test]
    fn roundtrips_through_json_value() {
        let p = sample();
        let v = p.to_value();
        assert_eq!(v["source_type"], "document");
        assert_eq!(v["page"], 4);
        assert!(v.get("fact").is_none());
        assert_eq!(ChunkPayload::from_value(&v).unwrap(), p);
    }

    #[test]
    fn document_and_meeting_citations() {
        let c = sample().citation();
        assert_eq!(c.kind, CitationKind::Document);
        assert_eq!(c.label, "Q3 Plan, p. 4");
        assert_eq!(c.page, Some(4));

        let mut t = sample();
        t.source_type = SourceType::Transcript;
        t.page = None;
        t.title = Some("Weekly sync".into());
        t.meeting_id = Some("room-1".into());
        t.start_ms = Some(754_000);
        t.speakers = vec!["Ann".into()];
        let c = t.citation();
        assert_eq!(c.kind, CitationKind::Transcript);
        assert_eq!(c.label, "Weekly sync @ 12:34");
        assert_eq!(c.ref_id.as_deref(), Some("room-1"));
        assert_eq!(c.speaker.as_deref(), Some("Ann"));
        assert_eq!(format_timestamp(3_725_000), "1:02:05");
    }

    #[test]
    fn id_normalisation() {
        let u = Uuid::now_v7();
        assert_eq!(normalize_id(&u.simple().to_string()), u.to_string());
        assert_eq!(normalize_id(" room-7 "), "room-7");
    }
}
