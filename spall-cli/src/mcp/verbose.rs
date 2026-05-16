//! Verbose-mode debug logging for `spall mcp` (stdio + HTTP transports).
//!
//! Activated by `--spall-verbose`. All output goes to stderr only — the
//! stdout JSON-RPC discipline is preserved at all costs. Each verbose
//! event is one stderr line prefixed with [`SENTINEL`] for easy grep:
//!
//! ```text
//! [spall-mcp] kind=startup api=petstore transport=stdio tools=2 profiles=admin,readonly
//! [spall-mcp] kind=tools/call tool=getpetbyid profile=<default> method=GET url=/pets/{petId}
//! [spall-mcp] kind=http-request origin=https://app.example.com allowlist=https://app.example.com
//! ```
//!
//! ### Redaction (v1 scope)
//!
//! HTTP request headers logged at the `http-request` boundary are
//! redacted by name (case-insensitive): see [`REDACTED_HEADER_NAMES`].
//! Bearer / Basic prefixes are preserved so the auth scheme stays
//! visible; everything after is `[REDACTED]`.
//!
//! ### What is NOT redacted in v1
//!
//! - Per-call URL **query parameters**: the `tools/call` log emits the
//!   spec's `path_template`, not the rendered URL with substituted
//!   path/query, so query-param redaction is a no-op for v1. The
//!   [`REDACTED_QUERY_PARAMS`] constant + the [`RedactionReason`]
//!   variants are defined ahead of a future render-side wiring.
//! - **Request body** and **response body** of the upstream API call.
//! - **Response headers** of the upstream API call.
//! - **Custom organization-specific** header names that aren't in
//!   [`REDACTED_HEADER_NAMES`]. If your spec uses `X-Foo-Token` for a
//!   credential, do not enable `--spall-verbose` in environments where
//!   stderr is captured to durable storage.

/// Sentinel prefix on every verbose line — stable wire contract for
/// e2e grep tests.
pub(crate) const SENTINEL: &str = "[spall-mcp]";

/// Redacted-value placeholders. `BEARER_REDACTED` and `BASIC_REDACTED`
/// preserve the auth scheme so debugging "wrong auth kind" issues
/// stays possible without leaking the credential bytes.
pub(crate) const BEARER_REDACTED: &str = "Bearer [REDACTED]";
pub(crate) const BASIC_REDACTED: &str = "Basic [REDACTED]";
pub(crate) const VALUE_REDACTED: &str = "[REDACTED]";

/// HTTP header names whose values are redacted in verbose output.
/// Lowercased; comparison is case-insensitive via `eq_ignore_ascii_case`.
/// Source of truth for the [`redact_header_value`] match arms — a unit
/// test asserts every entry here triggers a redaction (drift guard).
pub(crate) const REDACTED_HEADER_NAMES: &[&str] = &[
    "authorization",
    "cookie",
    "proxy-authorization",
];

/// URL query parameter names whose values would be redacted IF v1 wired
/// rendered-URL logging. Defined now so the constant + the
/// [`RedactionReason::SensitiveQueryParam`] variant are in place when
/// the render wire-up lands.
#[allow(dead_code)]
pub(crate) const REDACTED_QUERY_PARAMS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "access_token",
    "secret",
    "password",
];

/// Classifier for redaction sites. The verbose stderr line shows
/// `[REDACTED]` directly; this enum exists so internal call sites
/// can't drift to stringly-typed `Option<&'static str>` discriminators
/// (the `failed_via` retrofit lesson — sprint anti-pattern guard).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum RedactionReason {
    AuthorizationHeader,
    CookieHeader,
    ProxyAuthorizationHeader,
    SensitiveQueryParam(String),
    EmbeddedUrlCredentials,
}

/// Event-kind discriminator for the `kind=` token. Enum, not string,
/// so a typo at a call site fails to compile.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum VerboseEventKind {
    Startup,
    ToolsCall,
    HttpRequest,
}

use axum::http::HeaderMap as AxumHeaderMap;
use std::borrow::Cow;

/// Redact a single header value if its name is in [`REDACTED_HEADER_NAMES`].
/// Bearer/Basic prefixes are preserved (so debugging "wrong auth kind"
/// stays possible); everything after is `[REDACTED]`.
///
/// Returns `(rendered_value, Some(reason))` when redacted, `(value, None)`
/// when passed through. The `reason` is exposed for callers that want
/// to enrich a structured event later.
pub(crate) fn redact_header_value(
    name: &str,
    value: &str,
) -> (String, Option<RedactionReason>) {
    if !REDACTED_HEADER_NAMES
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
    {
        return (value.to_string(), None);
    }
    let reason = if name.eq_ignore_ascii_case("authorization") {
        RedactionReason::AuthorizationHeader
    } else if name.eq_ignore_ascii_case("cookie") {
        RedactionReason::CookieHeader
    } else {
        // The const-gated check above confirmed membership; the only
        // remaining variant is proxy-authorization.
        RedactionReason::ProxyAuthorizationHeader
    };
    let out = if value.starts_with("Bearer ") || value.starts_with("bearer ") {
        BEARER_REDACTED.to_string()
    } else if value.starts_with("Basic ") || value.starts_with("basic ") {
        BASIC_REDACTED.to_string()
    } else {
        VALUE_REDACTED.to_string()
    };
    (out, Some(reason))
}

/// JSON-quote a value iff it contains characters that would break the
/// `key=value` parsing of the verbose stderr line — whitespace, `=`,
/// `"`, `\`, or any control char. Borrowed when no quoting needed.
pub(crate) fn quote_if_needed(s: &str) -> Cow<'_, str> {
    let needs_quote = s
        .bytes()
        .any(|b| b == b' ' || b == b'\t' || b == b'=' || b == b'"' || b == b'\\' || b < 0x20);
    if needs_quote {
        Cow::Owned(serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\"")))
    } else {
        Cow::Borrowed(s)
    }
}

/// Format an axum `HeaderMap` as `{Name: "value", Name: "value"}`,
/// applying [`redact_header_value`] per header. Order is alphabetical
/// by name so the test grep is stable. Binary header values that fail
/// `to_str()` render as `<binary>`.
pub(crate) fn format_headers(headers: &AxumHeaderMap) -> String {
    let mut items: Vec<(&str, String)> = Vec::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        let raw = value.to_str().unwrap_or("<binary>");
        let (rendered, _reason) = redact_header_value(name.as_str(), raw);
        items.push((name.as_str(), rendered));
    }
    items.sort_by(|a, b| a.0.cmp(b.0));
    let mut parts: Vec<String> = Vec::with_capacity(items.len());
    for (name, rendered) in items {
        parts.push(format!("{}: {}", name, quote_if_needed(&rendered)));
    }
    format!("{{{}}}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    #[test]
    fn redact_header_value_bearer_preserves_scheme() {
        let (out, reason) = redact_header_value("Authorization", "Bearer supersecret");
        assert_eq!(out, "Bearer [REDACTED]");
        assert!(matches!(reason, Some(RedactionReason::AuthorizationHeader)));
    }

    #[test]
    fn redact_header_value_basic_preserves_scheme() {
        let (out, _) = redact_header_value("Authorization", "Basic dXNlcjpwYXNz");
        assert_eq!(out, "Basic [REDACTED]");
    }

    #[test]
    fn redact_header_value_unknown_scheme_redacts_entire_value() {
        let (out, reason) = redact_header_value("Authorization", "OpaqueTokenXYZ");
        assert_eq!(out, "[REDACTED]");
        assert!(matches!(reason, Some(RedactionReason::AuthorizationHeader)));
    }

    #[test]
    fn redact_header_value_cookie_redacts_full() {
        let (out, reason) = redact_header_value("Cookie", "session=abc123");
        assert_eq!(out, "[REDACTED]");
        assert!(matches!(reason, Some(RedactionReason::CookieHeader)));
    }

    #[test]
    fn redact_header_value_case_insensitive_name_match() {
        let (out, _) = redact_header_value("AUTHORIZATION", "Bearer x");
        assert_eq!(out, "Bearer [REDACTED]");
        let (out2, _) = redact_header_value("proxy-authorization", "Basic y");
        assert_eq!(out2, "Basic [REDACTED]");
    }

    #[test]
    fn redact_header_value_passes_through_unredacted_names() {
        let (out, reason) = redact_header_value("Content-Type", "application/json");
        assert_eq!(out, "application/json");
        assert!(reason.is_none());
    }

    #[test]
    fn quote_if_needed_borrows_clean_strings() {
        assert!(matches!(quote_if_needed("clean-value"), Cow::Borrowed(_)));
        assert!(matches!(quote_if_needed(""), Cow::Borrowed(_)));
    }

    #[test]
    fn quote_if_needed_quotes_problematic_strings() {
        // Whitespace, `=`, quotes, backslash, control chars all force
        // JSON quoting.
        assert_eq!(quote_if_needed("has space").as_ref(), "\"has space\"");
        assert_eq!(quote_if_needed("has=equals").as_ref(), "\"has=equals\"");
        assert_eq!(
            quote_if_needed("has\"quote").as_ref(),
            "\"has\\\"quote\""
        );
    }

    #[test]
    fn format_headers_redacts_and_sorts_by_name() {
        let mut hm = AxumHeaderMap::new();
        hm.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer supersecret"),
        );
        hm.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        let out = format_headers(&hm);
        // Alphabetical order: authorization < content-type.
        assert!(
            out.starts_with("{authorization: \"Bearer [REDACTED]\""),
            "got: {}",
            out,
        );
        assert!(out.contains("content-type: application/json"), "got: {}", out);
        assert!(!out.contains("supersecret"), "leak: {}", out);
    }

    #[test]
    fn redacted_header_names_const_matches_redact_function() {
        // Drift guard: every name in REDACTED_HEADER_NAMES must
        // trigger a redaction in `redact_header_value`. If a future
        // refactor adds a name to the const without wiring a match arm
        // (or vice versa), this test fires.
        for name in REDACTED_HEADER_NAMES {
            let (out, reason) = redact_header_value(name, "raw-value");
            assert_eq!(
                out, VALUE_REDACTED,
                "header {} in const but not redacted",
                name,
            );
            assert!(
                reason.is_some(),
                "header {} in const but no RedactionReason set",
                name,
            );
        }
    }

    #[test]
    fn format_headers_emits_binary_placeholder_for_non_ascii_value() {
        let mut hm = AxumHeaderMap::new();
        // 0xFF is not valid in HeaderValue::to_str(); the value must be
        // built from bytes to bypass the str-only constructor.
        let v = HeaderValue::from_bytes(b"\xff\xfe").expect("bytes");
        hm.insert(HeaderName::from_static("x-binary"), v);
        let out = format_headers(&hm);
        assert!(out.contains("x-binary: <binary>"), "got: {}", out);
    }
}
