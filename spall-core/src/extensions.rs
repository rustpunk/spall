//! Parse `x-cli-*` OpenAPI extensions (Restish-compatible).
//!
//! Supported vocabulary:
//! - `x-cli-name`    → override operation/parameter display name
//! - `x-cli-hidden`  → omit from help and command tree
//! - `x-cli-group`   → override tag grouping for operations

use crate::ir::{ResolvedOperation, ResolvedParameter};
use crate::value::SpallValue;

/// Parsed CLI-oriented extensions for an operation or parameter.
#[derive(Debug, Clone, Default)]
pub struct CliExtensions {
    pub cli_name: Option<String>,
    pub hidden: bool,
    pub group: Option<String>,
}

impl CliExtensions {
    pub fn from_operation(op: &ResolvedOperation) -> Self {
        let mut ext = Self::default();
        if let Some(SpallValue::Str(s)) = op.extensions.get("x-cli-name") {
            if is_valid_id(s) {
                ext.cli_name = Some(s.clone());
            } else {
                eprintln!(
                    "Warning: x-cli-name '{}' is not a valid subcommand name; using default '{}'",
                    s, op.operation_id
                );
            }
        }
        if let Some(SpallValue::Bool(b)) = op.extensions.get("x-cli-hidden") {
            ext.hidden = *b;
        }
        if let Some(SpallValue::Str(s)) = op.extensions.get("x-cli-group") {
            ext.group = Some(s.clone());
        }
        ext
    }

    pub fn from_parameter(param: &ResolvedParameter) -> Self {
        let mut ext = Self::default();
        if let Some(SpallValue::Str(s)) = param.extensions.get("x-cli-name") {
            if is_valid_id(s) {
                ext.cli_name = Some(s.clone());
            } else {
                eprintln!(
                    "Warning: x-cli-name '{}' is not a valid argument name; using default '{}'",
                    s, param.name
                );
            }
        }
        if let Some(SpallValue::Bool(b)) = param.extensions.get("x-cli-hidden") {
            ext.hidden = *b;
        }
        ext
    }

    /// Return the effective subcommand / display name.
    pub fn display_name(fallback: &str, ext: &Self) -> String {
        ext.cli_name.as_deref().unwrap_or(fallback).to_string()
    }
}

/// Basic validation: no spaces or shell-sensitive characters.
fn is_valid_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with('-') {
        return false;
    }
    let bad: &[char] = &[' ', '\t', '\n', '\r', '/', '\\', '\'', '"'];
    !s.contains(bad)
}