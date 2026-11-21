use serde_json::Value;

use crate::filter::Filter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distance {
    Cosine,
    Dot,
    Euclid,
}

impl Distance {
    pub fn as_qdrant(self) -> &'static str {
        match self {
            Self::Cosine => "Cosine",
            Self::Dot => "Dot",
            Self::Euclid => "Euclid",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cosine" => Some(Self::Cosine),
            "dot" => Some(Self::Dot),
            "euclid" => Some(Self::Euclid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadIndexType {
    Keyword,
    Integer,
    Bool,
    Text,
}

impl PayloadIndexType {
    pub fn as_qdrant(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Integer => "integer",
            Self::Bool => "bool",
            Self::Text => "text",
        }
    }
}

/// How to create a collection.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionSpec {
    pub dims: usize,
    pub distance: Distance,
    /// Payload fields to index (filtered fields should always be indexed).
    pub payload_indexes: Vec<(String, PayloadIndexType)>,
}

impl CollectionSpec {
    pub fn new(dims: usize, distance: Distance) -> Self {
        Self {
            dims,
            distance,
            payload_indexes: Vec::new(),
        }
    }

    pub fn with_index(mut self, field: impl Into<String>, kind: PayloadIndexType) -> Self {
        self.payload_indexes.push((field.into(), kind));
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectionInfo {
    pub dims: usize,
    pub points: u64,
}

/// A point to upsert. `id` must be a UUID string (or unsigned integer string) for Qdrant.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredPoint {
    pub id: String,
    pub score: f32,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchRequest {
    pub vector: Vec<f32>,
    pub limit: usize,
    pub filter: Option<Filter>,
    /// Drop hits scoring below this (store side).
    pub score_threshold: Option<f32>,
}

impl SearchRequest {
    pub fn new(vector: Vec<f32>, limit: usize) -> Self {
        Self {
            vector,
            limit,
            filter: None,
            score_threshold: None,
        }
    }

    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }
}
