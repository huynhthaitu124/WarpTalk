use serde_json::Value;

/// Semantic validation of a settings document (ranges, cross-field rules).
pub trait Validate {
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Range check helper producing a readable message.
pub fn check_range<N: PartialOrd + std::fmt::Display>(
    name: &str,
    v: N,
    min: N,
    max: N,
) -> Result<(), String> {
    if v < min || v > max {
        Err(format!("{name} must be within {min}..={max}, got {v}"))
    } else {
        Ok(())
    }
}

/// Deep-merge `overlay` onto `base`: objects merge key-wise, scalars and arrays replace.
/// A `null` in the overlay means "no override" and is ignored.
pub fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (_, Value::Null) => {}
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) => deep_merge(existing, v),
                    None if v.is_null() => {}
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_objects_replaces_scalars_and_arrays() {
        let mut base = json!({"a": 1, "o": {"x": 1, "y": 2}, "l": [1, 2]});
        deep_merge(
            &mut base,
            &json!({"a": 2, "o": {"y": 3}, "l": [9], "n": null}),
        );
        assert_eq!(base, json!({"a": 2, "o": {"x": 1, "y": 3}, "l": [9]}));
    }

    #[test]
    fn range_message() {
        assert!(check_range("k", 5, 1, 10).is_ok());
        assert!(check_range("k", 11, 1, 10).unwrap_err().contains("k"));
    }
}
