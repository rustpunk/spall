//! YAML parser chokepoint.
//!
//! This module is the **single** entry point for YAML parsing/serialization
//! in spall. No other code path is permitted to call `serde_saphyr::*`
//! directly. Two reasons:
//!
//! 1. **Bus-factor mitigation.** `serde-saphyr` is pre-1.0. If we ever need
//!    to fork or replace it, this file is the only thing that changes.
//! 2. **DoS-defense chokepoint.** A single [`Budget`] keeps depth / size /
//!    alias-expansion limits aligned with the threat model. Tightening the
//!    limits is a one-file edit.

use serde::Deserialize;
use serde::Serialize;

/// Hard 32 MB ceiling on YAML input. Covers the largest plausible
/// OpenAPI spec by ~3x. Enforced *before* parsing begins.
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;

/// Wrapper around [`serde_saphyr::Error`] so callers do not have to
/// import the underlying crate.
#[derive(Debug)]
pub struct YamlError(pub serde_saphyr::Error);

impl std::fmt::Display for YamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for YamlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<serde_saphyr::Error> for YamlError {
    fn from(e: serde_saphyr::Error) -> Self {
        Self(e)
    }
}

/// Construct the canonical [`serde_saphyr::Options`] used for *every*
/// parse in spall.
fn budget_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_reader_input_bytes: Some(MAX_INPUT_BYTES),
            max_depth: 256,
            max_inclusion_depth: 0,
            max_nodes: 1_000_000,
            enforce_alias_anchor_ratio: true,
        },
    }
}

/// Parse a YAML string into a typed value, with the canonical Budget
/// applied. **The** chokepoint — every YAML parse in spall MUST go
/// through here.
pub fn from_str<'de, T>(yaml: &'de str) -> Result<T, YamlError>
where
    T: Deserialize<'de>,
{
    if yaml.len() > MAX_INPUT_BYTES {
        return Err(YamlError(make_oversize_error(yaml.len())));
    }
    serde_saphyr::from_str_with_options(yaml, budget_options()).map_err(YamlError)
}

/// Serialize a value back to YAML. Thin pass-through; no budget needed
/// on the serialize side.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, String> {
    serde_saphyr::to_string(value).map_err(|e| e.to_string())
}

/// Build a synthetic oversize error. `serde_saphyr::Error` does not
/// expose a public constructor, so we coerce one out of the parser.
fn make_oversize_error(actual: usize) -> serde_saphyr::Error {
    let stub = format!(
        "# input exceeded {} bytes (actual {} bytes)\n: not_a_number",
        MAX_INPUT_BYTES, actual
    );
    serde_saphyr::from_str::<u8>(&stub).unwrap_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Serialize, Debug, PartialEq)]
    struct Trivial {
        name: String,
        count: u32,
    }

    #[test]
    fn basic_roundtrip() {
        let yaml = "name: hello\ncount: 7\n";
        let parsed: Trivial = from_str(yaml).expect("trivial parse");
        assert_eq!(parsed.name, "hello");
        assert_eq!(parsed.count, 7);

        let out = to_string(&parsed).expect("serialize");
        assert!(out.contains("name: hello"));
        assert!(out.contains("count: 7"));
    }

    #[test]
    fn budget_rejects_deep_nesting() {
        let yaml = format!("{}1{}\n", "[".repeat(400), "]".repeat(400));
        let res: Result<serde_json::Value, _> = from_str(&yaml);
        assert!(res.is_err(), "expected budget error on 400-level flow nesting");
    }

    #[test]
    fn budget_rejects_oversize_input() {
        let big = format!("name: {}\ncount: 1\n", "x".repeat(MAX_INPUT_BYTES + 1));
        let res: Result<Trivial, _> = from_str(&big);
        assert!(res.is_err(), "expected budget error on oversize input");
    }

    #[test]
    fn budget_rejects_billion_laughs() {
        let yaml = r#"
a: &a ["lol","lol","lol","lol","lol","lol","lol","lol","lol","lol"]
b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a,*a]
c: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b,*b]
d: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c,*c]
e: &e [*d,*d,*d,*d,*d,*d,*d,*d,*d,*d]
f: &f [*e,*e,*e,*e,*e,*e,*e,*e,*e,*e]
g: &g [*f,*f,*f,*f,*f,*f,*f,*f,*f,*f]
h: &h [*g,*g,*g,*g,*g,*g,*g,*g,*g,*g]
"#;
        let res: Result<serde_json::Value, _> = from_str(yaml);
        assert!(res.is_err(), "expected budget error on billion-laughs alias bomb");
    }

    #[test]
    fn budget_disables_include() {
        let yaml = "value: !include other.yaml\n";
        #[derive(Deserialize)]
        struct V {
            value: String,
        }
        let parsed: Result<V, _> = from_str(yaml);
        match parsed {
            Ok(v) => assert_eq!(v.value, "other.yaml"),
            Err(_) => { /* also acceptable: unknown tag rejection */ }
        }
    }
}
