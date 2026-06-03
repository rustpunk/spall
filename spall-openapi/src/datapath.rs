//! Where the array of items lives inside a response document.
//!
//! A [`DataPath`] is a tiny, fully-buffered value: either "the top-level value
//! is the array" or "the array lives at this RFC-6901 JSON pointer". It is
//! parsed once from configuration / vendor extension and then consulted by the
//! streaming skimmer to navigate to the array without materializing the
//! document.

use thiserror::Error;

/// The location of the item array within a response body.
///
/// Why: paginated APIs disagree on where the array lives — some return a bare
/// top-level array, others wrap it under keys like `result.items`. `DataPath`
/// captures both shapes so the skimmer can seek to the array regardless. The
/// *sourcing precedence* (vendor extension vs. config) is #27's concern; this
/// type only models the location and parses it.
///
/// Memory model: fully buffered and tiny — either a unit variant or a small
/// `Vec<String>` of pointer segments. No streaming.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DataPath {
    /// The response's top-level value is the item array itself.
    ///
    /// This is the default: an empty or `"/"` pointer means "the document root
    /// is the array".
    #[default]
    TopLevel,
    /// The item array lives under this sequence of object keys, e.g.
    /// `Pointer(["result", "items"])` for the array at `/result/items`.
    Pointer(Vec<String>),
}

/// An error parsing a JSON-pointer string into a [`DataPath`].
///
/// Why: RFC-6901 constrains pointer syntax, and we reject malformed input
/// rather than silently misinterpreting it (which could send the skimmer to the
/// wrong place). The message carries the offending pointer for diagnostics.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DataPathError {
    /// A non-empty pointer did not start with `/`, violating RFC-6901.
    #[error("JSON pointer must start with '/': {0:?}")]
    MissingLeadingSlash(String),
    /// A segment contained a `~` that was not part of a valid `~0`/`~1` escape.
    #[error("invalid '~' escape in JSON pointer segment: {0:?}")]
    InvalidEscape(String),
}

impl DataPath {
    /// Parses an RFC-6901 JSON-pointer string into a [`DataPath`].
    ///
    /// Why: configuration and the `x-spall-data-path` vendor extension express
    /// the array location as a pointer string; this turns that string into the
    /// structured location the skimmer navigates. Decodes the RFC-6901 escapes
    /// `~1` -> `/` and `~0` -> `~`. An empty string or `"/"` yields
    /// [`DataPath::TopLevel`]; `"/result/items"` yields
    /// `Pointer(["result", "items"])`.
    ///
    /// Note on `"/"`: a single slash is one empty reference token in strict
    /// RFC-6901, but spall treats it as a user shorthand for "top level" (no
    /// real API keys on an empty string), matching the documented contract that
    /// `""` or `"/"` mean [`DataPath::TopLevel`].
    ///
    /// # Errors
    /// Returns [`DataPathError::MissingLeadingSlash`] if a non-empty pointer
    /// does not begin with `/`, and [`DataPathError::InvalidEscape`] if a `~`
    /// is not followed by `0` or `1`.
    ///
    /// Memory model: allocates only the small segment vector; does no I/O.
    #[must_use = "parsing a data path returns a Result that must be handled"]
    pub fn from_pointer(pointer: &str) -> Result<DataPath, DataPathError> {
        // Empty or a lone slash => the top-level value is the array.
        if pointer.is_empty() || pointer == "/" {
            return Ok(DataPath::TopLevel);
        }
        if !pointer.starts_with('/') {
            return Err(DataPathError::MissingLeadingSlash(pointer.to_string()));
        }

        let mut segments = Vec::new();
        // Skip the leading '/', then each '/'-separated reference token.
        for raw in pointer[1..].split('/') {
            segments.push(decode_segment(raw)?);
        }
        Ok(DataPath::Pointer(segments))
    }

    /// Reads the `x-spall-data-path` vendor extension off an operation, if it
    /// names a valid JSON pointer.
    ///
    /// Why: an operation can declare where its item array lives directly in the
    /// spec via the `x-spall-data-path` extension (a JSON-pointer string). This
    /// reads that extension — mirroring spall-core's `x-cli-*` extension
    /// pattern — and parses it with [`DataPath::from_pointer`].
    ///
    /// Returns `None` when the extension is absent, is not a string, or is a
    /// malformed pointer. It never panics on bad spec input: a malformed pointer
    /// is treated the same as "no override", so a lower-precedence source can
    /// supply the data path instead.
    ///
    /// This is **one tier** of data-path sourcing. The full precedence — an
    /// explicit caller override, then `x-spall-data-path`, then the
    /// `ApiEntry.data_path` config field, then [`DataPath::TopLevel`] — plus the
    /// new config field, is assembled in spall-cli (#28); this crate stays free
    /// of spall-config and only knows how to read the vendor extension.
    ///
    /// Memory model: fully buffered and tiny; reads only the operation's small
    /// extension map and does no I/O.
    #[must_use]
    pub fn from_operation(op: &spall_core::ir::ResolvedOperation) -> Option<DataPath> {
        if let Some(spall_core::value::SpallValue::Str(s)) = op.extensions.get("x-spall-data-path")
        {
            DataPath::from_pointer(s).ok()
        } else {
            None
        }
    }
}

/// Decodes a single RFC-6901 reference token, expanding `~1` -> `/` and
/// `~0` -> `~`. A `~` followed by anything else is an error.
fn decode_segment(raw: &str) -> Result<String, DataPathError> {
    if !raw.contains('~') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some('0') => out.push('~'),
                Some('1') => out.push('/'),
                _ => return Err(DataPathError::InvalidEscape(raw.to_string())),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_top_level() {
        assert_eq!(DataPath::default(), DataPath::TopLevel);
    }

    #[test]
    fn empty_and_slash_are_top_level() {
        assert_eq!(DataPath::from_pointer(""), Ok(DataPath::TopLevel));
        assert_eq!(DataPath::from_pointer("/"), Ok(DataPath::TopLevel));
    }

    #[test]
    fn single_segment() {
        assert_eq!(
            DataPath::from_pointer("/root"),
            Ok(DataPath::Pointer(vec!["root".to_string()]))
        );
    }

    #[test]
    fn multi_segment() {
        assert_eq!(
            DataPath::from_pointer("/result/items"),
            Ok(DataPath::Pointer(vec![
                "result".to_string(),
                "items".to_string()
            ]))
        );
    }

    #[test]
    fn decodes_escapes() {
        // ~1 -> '/', ~0 -> '~'
        assert_eq!(
            DataPath::from_pointer("/a~1b"),
            Ok(DataPath::Pointer(vec!["a/b".to_string()]))
        );
        assert_eq!(
            DataPath::from_pointer("/a~0b"),
            Ok(DataPath::Pointer(vec!["a~b".to_string()]))
        );
        // ~01 decodes left-to-right: ~0 -> '~', then literal '1'.
        assert_eq!(
            DataPath::from_pointer("/m~0~1n"),
            Ok(DataPath::Pointer(vec!["m~/n".to_string()]))
        );
    }

    #[test]
    fn missing_leading_slash_errors() {
        assert_eq!(
            DataPath::from_pointer("root"),
            Err(DataPathError::MissingLeadingSlash("root".to_string()))
        );
    }

    #[test]
    fn invalid_escape_errors() {
        assert_eq!(
            DataPath::from_pointer("/a~2b"),
            Err(DataPathError::InvalidEscape("a~2b".to_string()))
        );
        assert_eq!(
            DataPath::from_pointer("/trailing~"),
            Err(DataPathError::InvalidEscape("trailing~".to_string()))
        );
    }

    fn op_with_extensions(
        extensions: indexmap::IndexMap<String, spall_core::value::SpallValue>,
    ) -> spall_core::ir::ResolvedOperation {
        spall_core::ir::ResolvedOperation {
            operation_id: "op".into(),
            method: spall_core::ir::HttpMethod::Get,
            path_template: "/x".into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            responses: indexmap::IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions,
            servers: Vec::new(),
        }
    }

    #[test]
    fn from_operation_reads_x_spall_data_path() {
        let mut ext = indexmap::IndexMap::new();
        ext.insert(
            "x-spall-data-path".to_string(),
            spall_core::value::SpallValue::Str("/result/items".to_string()),
        );
        let op = op_with_extensions(ext);
        assert_eq!(
            DataPath::from_operation(&op),
            Some(DataPath::Pointer(vec![
                "result".to_string(),
                "items".to_string()
            ]))
        );
    }

    #[test]
    fn from_operation_top_level_pointer() {
        let mut ext = indexmap::IndexMap::new();
        ext.insert(
            "x-spall-data-path".to_string(),
            spall_core::value::SpallValue::Str("/".to_string()),
        );
        let op = op_with_extensions(ext);
        assert_eq!(DataPath::from_operation(&op), Some(DataPath::TopLevel));
    }

    #[test]
    fn from_operation_absent_is_none() {
        let op = op_with_extensions(indexmap::IndexMap::new());
        assert_eq!(DataPath::from_operation(&op), None);
    }

    #[test]
    fn from_operation_non_string_is_none() {
        let mut ext = indexmap::IndexMap::new();
        ext.insert(
            "x-spall-data-path".to_string(),
            spall_core::value::SpallValue::Bool(true),
        );
        let op = op_with_extensions(ext);
        assert_eq!(DataPath::from_operation(&op), None);
    }

    #[test]
    fn from_operation_malformed_pointer_is_none() {
        // Missing leading slash => from_pointer errors => None, no panic.
        let mut ext = indexmap::IndexMap::new();
        ext.insert(
            "x-spall-data-path".to_string(),
            spall_core::value::SpallValue::Str("result/items".to_string()),
        );
        let op = op_with_extensions(ext);
        assert_eq!(DataPath::from_operation(&op), None);
    }
}
