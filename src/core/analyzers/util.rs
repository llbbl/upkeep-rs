//! Shared utilities for analyzer modules.

use serde_json::Value;

/// Produces a human-readable description of a JSON value's schema.
///
/// This is useful for error messages when encountering unexpected JSON structures
/// from external tools like `cargo geiger` or `cargo machete`.
///
/// # Examples
///
/// ```ignore
/// use serde_json::json;
/// use crate::core::analyzers::util::describe_json_schema;
///
/// assert_eq!(describe_json_schema(&json!(null)), "null");
/// assert_eq!(describe_json_schema(&json!({"foo": 1})), "object(keys=[foo])");
/// ```
pub fn describe_json_schema(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => format!("array(len={})", items.len()),
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            format!("object(keys=[{}])", keys.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn describe_null() {
        assert_eq!(describe_json_schema(&json!(null)), "null");
    }

    #[test]
    fn describe_boolean() {
        assert_eq!(describe_json_schema(&json!(true)), "boolean");
        assert_eq!(describe_json_schema(&json!(false)), "boolean");
    }

    #[test]
    fn describe_number() {
        assert_eq!(describe_json_schema(&json!(42)), "number");
        assert_eq!(describe_json_schema(&json!(3.14)), "number");
    }

    #[test]
    fn describe_string() {
        assert_eq!(describe_json_schema(&json!("hello")), "string");
    }

    #[test]
    fn describe_array() {
        assert_eq!(describe_json_schema(&json!([])), "array(len=0)");
        assert_eq!(describe_json_schema(&json!([1, 2, 3])), "array(len=3)");
    }

    #[test]
    fn describe_object() {
        assert_eq!(describe_json_schema(&json!({})), "object(keys=[])");
        assert_eq!(
            describe_json_schema(&json!({"b": 1, "a": 2})),
            "object(keys=[a, b])"
        );
    }
}
