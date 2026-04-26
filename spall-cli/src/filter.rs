//! Response filtering via JMESPath.

use jmespath::ToJmespath;

/// Evaluate a JMESPath expression against a JSON value.
///
/// On error, returns the original message string so callers can warn and fall back.
pub fn filter_response(
    expr: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let parsed = jmespath::compile(expr).map_err(|e| format!("{}", e))?;
    let jmes_input = value
        .to_jmespath()
        .map_err(|e| format!("JMESPath input error: {}", e))?;
    let result = parsed
        .search(jmes_input)
        .map_err(|e| format!("JMESPath search error: {}", e))?;
    serde_json::to_value(&*result)
        .map_err(|e| format!("JMESPath serde error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_selector() {
        let value = serde_json::json!({"name": "test", "count": 42});
        let result = filter_response("name", &value).unwrap();
        assert_eq!(result, serde_json::json!("test"));
    }

    #[test]
    fn array_projection() {
        let value = serde_json::json!([{"name": "a"}, {"name": "b"}]);
        let result = filter_response("[].name", &value).unwrap();
        assert_eq!(result, serde_json::json!(["a", "b"]));
    }

    #[test]
    fn invalid_expr_warns() {
        let value = serde_json::json!({"x": 1});
        assert!(filter_response("@invalid!", &value).is_err());
    }
}
