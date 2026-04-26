//! Lightweight schema validator operating on `ResolvedSchema`.
//!
//! Intentionally avoids pulling in `jsonschema` — it evaluates raw JSON Schema
//! documents, and our IR already flattens `$ref` and enums.

use crate::ir::ResolvedSchema;
use crate::value::SpallValue;
use indexmap::IndexMap;
use std::sync::OnceLock;

/// A single validation failure with JSON-pointer style path.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    /// JSON pointer to the invalid value (e.g., "/body/name", "/param/petId").
    pub pointer: String,
    /// Human-readable message.
    pub message: String,
}

/// Validate a CLI parameter string against its schema.
pub fn validate_param(value: &str, schema: &ResolvedSchema) -> Result<(), ValidationError> {
    // Enum check
    if !schema.enum_values.is_empty() {
        let matched = schema.enum_values.iter().any(|ev| match ev {
            SpallValue::Str(s) => s == value,
            SpallValue::I64(i) => value.parse::<i64>().ok() == Some(*i),
            SpallValue::U64(u) => value.parse::<u64>().ok() == Some(*u),
            SpallValue::F64(f) => value
                .parse::<f64>()
                .ok()
                .map(|v| (v - *f).abs() < f64::EPSILON)
                .unwrap_or(false),
            SpallValue::Bool(b) => value.parse::<bool>().ok() == Some(*b),
            _ => false,
        });
        if !matched {
            return Err(ValidationError {
                pointer: "/param".to_string(),
                message: format!(
                    "value '{}' not in enum: {:?}",
                    value, schema.enum_values
                ),
            });
        }
    }

    match schema.type_name.as_deref() {
        Some("string") | None => {
            check_string(value, schema, "/param")?;
        }
        Some("integer") => {
            let num = value.parse::<i64>().map_err(|_| ValidationError {
                pointer: "/param".to_string(),
                message: format!("expected integer, got '{}'", value),
            })?;
            check_number(num as f64, schema, "/param")?;
        }
        Some("number") => {
            let f = value.parse::<f64>().map_err(|_| ValidationError {
                pointer: "/param".to_string(),
                message: format!("expected number, got '{}'", value),
            })?;
            check_number(f, schema, "/param")?;
        }
        Some("boolean") => {
            let _ = value.parse::<bool>().map_err(|_| ValidationError {
                pointer: "/param".to_string(),
                message: format!("expected boolean, got '{}'", value),
            })?;
        }
        Some("array") | Some("object") => {
            // CLI params are usually scalar; attempt JSON parse for rare cases.
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(value) {
                let errs = validate_value(&val, schema, "/param");
                if let Some(first) = errs.into_iter().next() {
                    return Err(first);
                }
            } else {
                return Err(ValidationError {
                    pointer: "/param".to_string(),
                    message: format!("expected JSON {}, got '{}'", schema.type_name.as_deref().unwrap_or("value"), value),
                });
            }
        }
        _ => {}
    }

    Ok(())
}

/// Validate a JSON body value recursively against its schema.
#[must_use]
pub fn validate_body(value: &serde_json::Value, schema: &ResolvedSchema) -> Vec<ValidationError> {
    validate_value(value, schema, "/body")
}

/// Recursively validate a JSON value, collecting all errors.
fn validate_value(
    value: &serde_json::Value,
    schema: &ResolvedSchema,
    pointer: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if value.is_null() && schema.nullable {
        return errors;
    }

    if !schema.enum_values.is_empty() {
        let sv = SpallValue::from(value);
        if !schema.enum_values.contains(&sv) {
            errors.push(ValidationError {
                pointer: pointer.to_string(),
                message: "value not in enum".to_string(),
            });
        }
    }

    match schema.type_name.as_deref() {
        Some("string") => {
            if let Some(s) = value.as_str() {
                if let Err(e) = check_string(s, schema, pointer) {
                    errors.push(e);
                }
            } else {
                errors.push(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!("expected string, got {}", json_type_name(value)),
                });
            }
        }
        Some("integer") => {
            if let Some(n) = value.as_i64() {
                if let Err(e) = check_number(n as f64, schema, pointer) {
                    errors.push(e);
                }
                if value.as_f64().map(|f| f.fract() != 0.0).unwrap_or(false) {
                    errors.push(ValidationError {
                        pointer: pointer.to_string(),
                        message: format!(
                            "expected integer, got {}",
                            json_type_name(value)
                        ),
                    });
                }
            } else {
                errors.push(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!(
                        "expected integer, got {}",
                        json_type_name(value)
                    ),
                });
            }
        }
        Some("number") => {
            if let Some(f) = value.as_f64() {
                if let Err(e) = check_number(f, schema, pointer) {
                    errors.push(e);
                }
            } else {
                errors.push(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!(
                        "expected number, got {}",
                        json_type_name(value)
                    ),
                });
            }
        }
        Some("boolean") if !value.is_boolean() => {
            errors.push(ValidationError {
                pointer: pointer.to_string(),
                message: format!(
                    "expected boolean, got {}",
                    json_type_name(value)
                ),
            });
        }
        Some("array") => {
            if let Some(arr) = value.as_array() {
                if let Some(min) = schema.min_items {
                    if arr.len() < min {
                        errors.push(ValidationError {
                            pointer: pointer.to_string(),
                            message: format!(
                                "array length {} < min_items {}",
                                arr.len(), min
                            ),
                        });
                    }
                }
                if let Some(max) = schema.max_items {
                    if arr.len() > max {
                        errors.push(ValidationError {
                            pointer: pointer.to_string(),
                            message: format!(
                                "array length {} > max_items {}",
                                arr.len(), max
                            ),
                        });
                    }
                }
                if schema.unique_items {
                    let mut seen = std::collections::HashSet::new();
                    for item in arr {
                        let key = serde_json::to_string(item).unwrap_or_default();
                        if !seen.insert(key) {
                            errors.push(ValidationError {
                                pointer: pointer.to_string(),
                                message: "array contains duplicate items".to_string(),
                            });
                            break;
                        }
                    }
                }
                if let Some(ref items_schema) = schema.items {
                    for (i, item) in arr.iter().enumerate() {
                        let item_pointer = format!("{}/{}", pointer, i);
                        errors.extend(validate_value(item, items_schema, &item_pointer));
                    }
                }
            } else {
                errors.push(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!("expected array, got {}", json_type_name(value)),
                });
            }
        }
        Some("object") => {
            if let Some(obj) = value.as_object() {
                for (prop_name, prop_schema) in &schema.properties {
                    if let Some(prop_val) = obj.get(prop_name) {
                        let prop_pointer = format!("{}/{}", pointer, prop_name);
                        errors.extend(validate_value(prop_val, prop_schema, &prop_pointer));
                    }
                }
                if !schema.additional_properties {
                    for key in obj.keys() {
                        if !schema.properties.contains_key(key) {
                            errors.push(ValidationError {
                                pointer: format!("{}/{}", pointer, key),
                                message: format!(
                                    "additional property '{}' not allowed",
                                    key
                                ),
                            });
                        }
                    }
                }
            } else {
                errors.push(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!("expected object, got {}", json_type_name(value)),
                });
            }
        }
        None => {
            // Untyped / any schema — no specific validation.
        }
        _ => {}
    }

    errors
}

fn check_string(value: &str, schema: &ResolvedSchema, pointer: &str) -> Result<(), ValidationError> {
    if let Some(ref pattern) = schema.pattern {
        if let Ok(re) = compiled_regex(pattern) {
            if !re.is_match(value) {
                return Err(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!(
                        "value '{}' does not match pattern '{}'",
                        value, pattern
                    ),
                });
            }
        }
    }
    if let Some(min) = schema.min_length {
        if value.len() < min {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!(
                    "string length {} < min_length {}",
                    value.len(), min
                ),
            });
        }
    }
    if let Some(max) = schema.max_length {
        if value.len() > max {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!(
                    "string length {} > max_length {}",
                    value.len(), max
                ),
            });
        }
    }
    Ok(())
}

fn check_number(value: f64, schema: &ResolvedSchema, pointer: &str) -> Result<(), ValidationError> {
    if let Some(min) = schema.minimum {
        if schema.exclusive_minimum && value <= min {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!("value {} <= exclusive_minimum {}", value, min),
            });
        }
        if !schema.exclusive_minimum && value < min {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!("value {} < minimum {}", value, min),
            });
        }
    }
    if let Some(max) = schema.maximum {
        if schema.exclusive_maximum && value >= max {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!("value {} >= exclusive_maximum {}", value, max),
            });
        }
        if !schema.exclusive_maximum && value > max {
            return Err(ValidationError {
                pointer: pointer.to_string(),
                message: format!("value {} > maximum {}", value, max),
            });
        }
    }
    if let Some(multiple) = schema.multiple_of {
        if multiple != 0.0 {
            let rem = value.rem_euclid(multiple);
            if rem > f64::EPSILON && (multiple - rem) > f64::EPSILON {
                return Err(ValidationError {
                    pointer: pointer.to_string(),
                    message: format!(
                        "value {} is not a multiple of {}",
                        value, multiple
                    ),
                });
            }
        }
    }
    Ok(())
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Lazily compile and cache regexes keyed by pattern string.
///
/// Uses a static mutex map. On cache miss, compiles the regex and stores
/// a clone. Returns an owned `regex::Regex` on success.
fn compiled_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    use std::sync::Mutex;

    static CACHE: OnceLock<Mutex<IndexMap<String, regex::Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(IndexMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(re) = guard.get(pattern) {
        return Ok(re.clone());
    }
    let re = regex::Regex::new(pattern)?;
    guard.insert(pattern.to_string(), re.clone());
    Ok(re)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::SpallValue;

    fn string_schema() -> ResolvedSchema {
        ResolvedSchema {
            type_name: Some("string".to_string()),
            format: None,
            description: None,
            default: None,
            enum_values: vec![],
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            multiple_of: None,
            exclusive_minimum: false,
            exclusive_maximum: false,
            min_items: None,
            max_items: None,
            unique_items: false,
            additional_properties: true,
            properties: IndexMap::new(),
            items: None,
        }
    }

    #[test]
    fn valid_string_passes() {
        let s = string_schema();
        assert!(validate_param("hello", &s).is_ok());
    }

    #[test]
    fn string_pattern_match() {
        let mut s = string_schema();
        s.pattern = Some(r"^\d+$".to_string());
        assert!(validate_param("123", &s).is_ok());
        assert!(validate_param("abc", &s).is_err());
    }

    #[test]
    fn string_length_bounds() {
        let mut s = string_schema();
        s.min_length = Some(2);
        s.max_length = Some(5);
        assert!(validate_param("ab", &s).is_ok());
        assert!(validate_param("hello", &s).is_ok());
        assert!(validate_param("h", &s).is_err());
        assert!(validate_param("hello!", &s).is_err());
    }

    #[test]
    fn integer_type_check() {
        let mut s = string_schema();
        s.type_name = Some("integer".to_string());
        assert!(validate_param("42", &s).is_ok());
        assert!(validate_param("3.14", &s).is_err());
        assert!(validate_param("foo", &s).is_err());
    }

    #[test]
    fn integer_range() {
        let mut s = string_schema();
        s.type_name = Some("integer".to_string());
        s.minimum = Some(0.0);
        s.maximum = Some(100.0);
        assert!(validate_param("50", &s).is_ok());
        assert!(validate_param("-1", &s).is_err());
        assert!(validate_param("101", &s).is_err());
    }

    #[test]
    fn number_type_check() {
        let mut s = string_schema();
        s.type_name = Some("number".to_string());
        assert!(validate_param("3.14", &s).is_ok());
        assert!(validate_param("42", &s).is_ok());
        assert!(validate_param("foo", &s).is_err());
    }

    #[test]
    fn boolean_type_check() {
        let mut s = string_schema();
        s.type_name = Some("boolean".to_string());
        assert!(validate_param("true", &s).is_ok());
        assert!(validate_param("false", &s).is_ok());
        assert!(validate_param("yes", &s).is_err());
    }

    #[test]
    fn enum_string_match() {
        let mut s = string_schema();
        s.enum_values = vec![
            SpallValue::Str("alpha".to_string()),
            SpallValue::Str("beta".to_string()),
        ];
        assert!(validate_param("alpha", &s).is_ok());
        assert!(validate_param("gamma", &s).is_err());
    }

    #[test]
    fn enum_integer_match() {
        let mut s = string_schema();
        s.type_name = Some("integer".to_string());
        s.enum_values = vec![SpallValue::I64(1), SpallValue::I64(2)];
        assert!(validate_param("1", &s).is_ok());
        assert!(validate_param("3", &s).is_err());
    }

    #[test]
    fn body_object_validation() {
        let mut inner = string_schema();
        inner.type_name = Some("integer".to_string());

        let mut props = IndexMap::new();
        props.insert("count".to_string(), inner);

        let schema = ResolvedSchema {
            type_name: Some("object".to_string()),
            properties: props,
            additional_properties: false,
            ..string_schema()
        };

        let value = serde_json::json!({ "count": 5 });
        assert!(validate_body(&value, &schema).is_empty());

        let bad = serde_json::json!({ "count": "five" });
        let errs = validate_body(&bad, &schema);
        assert!(!errs.is_empty());
        assert!(errs[0].pointer.contains("count"));
    }

    #[test]
    fn body_array_validation() {
        let item = ResolvedSchema {
            type_name: Some("string".to_string()),
            ..string_schema()
        };
        let schema = ResolvedSchema {
            type_name: Some("array".to_string()),
            items: Some(Box::new(item)),
            min_items: Some(1),
            max_items: Some(3),
            ..string_schema()
        };

        let ok = serde_json::json!(["a", "b"]);
        assert!(validate_body(&ok, &schema).is_empty());

        let too_long = serde_json::json!(["a", "b", "c", "d"]);
        let errs = validate_body(&too_long, &schema);
        assert!(!errs.is_empty());
    }

    #[test]
    fn nullable_allows_null() {
        let mut s = string_schema();
        s.nullable = true;
        let v = serde_json::Value::Null;
        assert!(validate_body(&v, &s).is_empty());
    }

    #[test]
    fn unique_items_check() {
        let item = ResolvedSchema {
            type_name: Some("integer".to_string()),
            ..string_schema()
        };
        let schema = ResolvedSchema {
            type_name: Some("array".to_string()),
            items: Some(Box::new(item)),
            unique_items: true,
            ..string_schema()
        };

        let ok = serde_json::json!([1, 2, 3]);
        assert!(validate_body(&ok, &schema).is_empty());

        let dup = serde_json::json!([1, 2, 1]);
        let errs = validate_body(&dup, &schema);
        assert!(!errs.is_empty());
    }
}