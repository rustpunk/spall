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
#[allow(dead_code)]
pub(crate) const BEARER_REDACTED: &str = "Bearer [REDACTED]";
#[allow(dead_code)]
pub(crate) const BASIC_REDACTED: &str = "Basic [REDACTED]";
#[allow(dead_code)]
pub(crate) const VALUE_REDACTED: &str = "[REDACTED]";

/// HTTP header names whose values are redacted in verbose output.
/// Lowercased; comparison is case-insensitive via `eq_ignore_ascii_case`.
#[allow(dead_code)]
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
