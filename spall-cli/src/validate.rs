//! Preflight validation of operation parameters and request bodies.

use spall_core::ir::{ResolvedOperation, ResolvedSchema};
use spall_core::validator::{validate_body, validate_param, ValidationError};

/// Validate all operation parameters before execution.
pub fn preflight_validate(
    op: &ResolvedOperation,
    phase2_matches: &clap::ArgMatches,
) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    for param in &op.parameters {
        let id = format!("{}-{}", param.location.as_str(), param.name);
        if let Some(value) = phase2_matches.get_one::<String>(&id) {
            if let Err(mut e) = validate_param(value, &param.schema) {
                e.pointer = format!("/param/{}", param.name);
                errors.push(e);
            }
        } else if param.required {
            errors.push(ValidationError {
                pointer: format!("/param/{}", param.name),
                message: format!("required parameter '{}' is missing", param.name),
            });
        }
    }

    // Validate JSON body if present and operation has a request body
    if op.request_body.is_some() {
        if let Some(values) = phase2_matches.get_many::<String>("data") {
            let parts: Vec<String> = values.cloned().collect();
            if let Some(last) = parts.last() {
                let data = if last == "-" {
                    // Cannot validate stdin preflight without reading it; skip.
                    String::new()
                } else if let Some(path) = last.strip_prefix('@') {
                    match std::fs::read_to_string(path) {
                        Ok(content) => content,
                        Err(e) => {
                            errors.push(ValidationError {
                                pointer: "/body".to_string(),
                                message: format!("cannot read body file '{}': {}", path, e),
                            });
                            String::new()
                        }
                    }
                } else {
                    last.clone()
                };

                if !data.is_empty() {
                    if let Some(ref body_def) = op.request_body {
                        if let Some(mt) = body_def.content.get("application/json") {
                            if let Some(ref schema) = mt.schema {
                                match serde_json::from_str::<serde_json::Value>(&data) {
                                    Ok(val) => {
                                        let body_errors = validate_body(&val, schema);
                                        for e in body_errors {
                                            errors.push(e);
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(ValidationError {
                                            pointer: "/body".to_string(),
                                            message: format!("invalid JSON body: {}", e),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate a response body against the 2xx response schema (if present).
///
/// Returns errors as warnings — callers should print them to stderr and
/// continue execution.
pub fn response_validate(
    op: &ResolvedOperation,
    status: u16,
    content_type: &str,
    body: &serde_json::Value,
) -> Vec<ValidationError> {
    if !status.to_string().starts_with('2') {
        return Vec::new();
    }

    let status_key = status.to_string();

    let Some(response) = op.responses.get(&status_key) else {
        return Vec::new();
    };

    // Normalise content type: strip charset, whitespace.
    let ct_clean = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();

    let Some(mt) = response.content.get(ct_clean)
        .or_else(|| response.content.get("application/json"))
        .or_else(|| response.content.values().next())
    else {
        return Vec::new();
    };

    if let Some(ref schema) = mt.schema {
        validate_body(body, schema)
    } else {
        Vec::new()
    }
}

/// Format validation errors for stderr.
pub fn format_errors(errors: &[ValidationError]) -> String {
    let mut lines = Vec::new();
    for err in errors {
        lines.push(format!("  {}: {}", err.pointer, err.message));
    }
    lines.join("\n")
}

/// Validate a raw CLI argument value against a schema for ad-hoc use.
pub fn validate_value_raw(
    value: &str,
    schema: &ResolvedSchema,
) -> Result<(), ValidationError> {
    validate_param(value, schema)
}
