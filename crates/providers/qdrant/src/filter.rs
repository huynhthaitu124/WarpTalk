use serde_json::{Value, json};

/// Qdrant-style filter. `should` non-empty means at least one `should` condition must hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub must: Vec<Condition>,
    pub must_not: Vec<Condition>,
    pub should: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// `key == value` (an array payload matches when it contains the value).
    Match {
        key: String,
        value: Value,
    },
    /// `key` equals any of `any` (empty `any` matches nothing).
    MatchAny {
        key: String,
        any: Vec<Value>,
    },
    /// Point id is one of these.
    HasId(Vec<String>),
    Nested(Box<Filter>),
}

impl Condition {
    pub fn eq(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Match {
            key: key.into(),
            value: value.into(),
        }
    }

    pub fn any_of<V: Into<Value>>(
        key: impl Into<String>,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        Self::MatchAny {
            key: key.into(),
            any: values.into_iter().map(Into::into).collect(),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Match { key, value } => json!({"key": key, "match": {"value": value}}),
            Self::MatchAny { key, any } => json!({"key": key, "match": {"any": any}}),
            Self::HasId(ids) => json!({"has_id": ids}),
            Self::Nested(f) => f.to_json(),
        }
    }

    fn matches(&self, id: &str, payload: &Value) -> bool {
        match self {
            Self::Match { key, value } => payload
                .get(key)
                .is_some_and(|actual| value_matches(actual, value)),
            Self::MatchAny { key, any } => payload
                .get(key)
                .is_some_and(|actual| any.iter().any(|v| value_matches(actual, v))),
            Self::HasId(ids) => ids.iter().any(|i| i == id),
            Self::Nested(f) => f.matches(id, payload),
        }
    }
}

fn scalar_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn value_matches(actual: &Value, wanted: &Value) -> bool {
    match actual {
        Value::Array(items) => items.iter().any(|i| scalar_eq(i, wanted)),
        other => scalar_eq(other, wanted),
    }
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.must.is_empty() && self.must_not.is_empty() && self.should.is_empty()
    }

    pub fn must(conditions: Vec<Condition>) -> Self {
        Self {
            must: conditions,
            ..Self::default()
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        if !self.must.is_empty() {
            m.insert(
                "must".into(),
                Value::Array(self.must.iter().map(Condition::to_json).collect()),
            );
        }
        if !self.must_not.is_empty() {
            m.insert(
                "must_not".into(),
                Value::Array(self.must_not.iter().map(Condition::to_json).collect()),
            );
        }
        if !self.should.is_empty() {
            m.insert(
                "should".into(),
                Value::Array(self.should.iter().map(Condition::to_json).collect()),
            );
        }
        Value::Object(m)
    }

    /// Evaluate against a point (used by the in-memory store).
    pub fn matches(&self, id: &str, payload: &Value) -> bool {
        self.must.iter().all(|c| c.matches(id, payload))
            && !self.must_not.iter().any(|c| c.matches(id, payload))
            && (self.should.is_empty() || self.should.iter().any(|c| c.matches(id, payload)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_and_matching() {
        let f = Filter {
            must: vec![Condition::eq("workspace_id", "w1")],
            must_not: vec![Condition::HasId(vec!["bad".into()])],
            should: vec![
                Condition::any_of("source_id", ["a", "b"]),
                Condition::Nested(Box::new(Filter::must(vec![Condition::eq("k", true)]))),
            ],
        };
        let j = f.to_json();
        assert_eq!(j["must"][0]["match"]["value"], "w1");
        assert_eq!(j["must_not"][0]["has_id"][0], "bad");
        assert_eq!(j["should"][0]["match"]["any"], json!(["a", "b"]));
        assert_eq!(j["should"][1]["must"][0]["key"], "k");

        let p = json!({"workspace_id": "w1", "source_id": "a"});
        assert!(f.matches("x", &p));
        assert!(!f.matches("bad", &p));
        assert!(!f.matches("x", &json!({"workspace_id": "w1", "source_id": "z"})));
        assert!(f.matches("x", &json!({"workspace_id": "w1", "k": true})));
    }

    #[test]
    fn empty_any_matches_nothing_and_arrays_contain() {
        let f = Filter::must(vec![Condition::any_of("s", Vec::<String>::new())]);
        assert!(!f.matches("x", &json!({"s": "a"})));
        let g = Filter::must(vec![Condition::eq("ids", "s1")]);
        assert!(g.matches("x", &json!({"ids": ["s0", "s1"]})));
        let n = Filter::must(vec![Condition::eq("page", 3)]);
        assert!(n.matches("x", &json!({"page": 3.0})));
    }
}
