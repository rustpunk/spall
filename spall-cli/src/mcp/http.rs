//! Streamable HTTP transport for `spall mcp`.
//!
//! Implements MCP spec 2025-06-18 §HTTP for the server side. Single
//! POST endpoint at `/` accepts JSON-RPC requests and replies with the
//! same JSON-RPC framing the stdio transport uses — the dispatch path
//! is shared via [`super::handle_line`].
//!
//! Wire contract:
//! - **POST /** with `Content-Type: application/json`. Body is one
//!   JSON-RPC 2.0 request frame. Response is one JSON-RPC frame as
//!   `application/json`. Streaming (text/event-stream) is documented
//!   in the spec for long-running responses; v1 tools are all
//!   request/response so we never produce SSE. Clients that send
//!   `Accept: text/event-stream` get JSON anyway — per spec they MUST
//!   accept both, and the server is free to pick the format.
//! - **`Mcp-Session-Id`** is issued on `initialize` and required on
//!   every subsequent request. Sessions live for the process's
//!   lifetime; restarting the server invalidates all sessions
//!   (this matches FastMCP / mcp-remote behavior; see Inspector #905
//!   and claude-code #27142 for the failure modes of getting this
//!   wrong).
//! - **Origin** validation kicks in when
//!   `--spall-allowed-origin <origin>` is set (repeatable). A request
//!   whose `Origin` header isn't in the allowlist gets `403 Forbidden`
//!   before the body is deserialized — mitigates the DNS-rebinding
//!   class the spec calls out.
//! - **Bind interface** defaults to `127.0.0.1`. The spec recommends
//!   localhost-only by default; `--spall-bind <addr>` opts into
//!   exposing the server on other interfaces.
//!
//! Out of scope (file a new issue if needed):
//! - Streaming responses via SSE (no long-running v1 tools).
//! - Server-initiated GET event stream for server→client notifications.
//! - TLS termination (reverse-proxy responsibility per the issue).
//! - Auth on the HTTP endpoint itself.

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use indexmap::IndexMap;
use rand::RngCore;
use serde_json::{json, Value};
use spall_core::ir::ResolvedSpec;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use super::{handle_line, AuthProfiles, ToolEntry};

/// Cap on request body size. axum's default is 2 MiB which is too
/// small for OpenAPI specs that legitimately have multipart uploads;
/// 16 MiB covers the common cases without inviting OOM via large
/// adversarial payloads. The matching configuration lives in
/// `axum::extract::DefaultBodyLimit`.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Header name for the MCP session identifier, per spec.
const HEADER_SESSION_ID: &str = "mcp-session-id";

/// Default port when `--spall-port` is omitted. Picked from the
/// dynamic / unassigned range to avoid colliding with anything common.
pub const DEFAULT_PORT: u16 = 8765;

/// Default bind interface — localhost only, mitigating DNS rebinding.
/// Users opt into broader binds with `--spall-bind <addr>`.
pub const DEFAULT_BIND: &str = "127.0.0.1";

/// Run the Streamable HTTP transport until the server's task ends
/// (typically: process is killed externally).
///
/// `listen_addr` is the bound socket; pass port `0` to let the kernel
/// pick a free port. The bound port is logged to stderr on a sentinel
/// line that e2e tests grep for: `[spall-mcp] listening on http://...`.
#[must_use = "the server's Result carries network and protocol errors"]
#[allow(clippy::too_many_arguments)]
pub async fn run_http(
    api_name: String,
    spec: ResolvedSpec,
    profiles: AuthProfiles,
    include: Vec<String>,
    exclude: Vec<String>,
    max_tools: Option<usize>,
    auth_tool: HashMap<String, String>,
    listen_addr: SocketAddr,
    allowed_origins: Vec<String>,
) -> Result<(), crate::SpallCliError> {
    let registry = super::prepare_server(
        &api_name,
        &format!("http (listening on {})", listen_addr),
        &spec,
        &include,
        &exclude,
        max_tools,
        &auth_tool,
    );

    let state = Arc::new(HttpState {
        spec,
        profiles,
        registry,
        sessions: RwLock::new(HashSet::new()),
        allowed_origins: allowed_origins.into_iter().collect(),
    });

    // Align the CORS layer with the handler-side Origin allowlist so
    // browser preflight rejection matches the actual POST rejection.
    // When no allowlist is set, fall back to the localhost-only
    // permissive policy — same machine the default 127.0.0.1 bind
    // reaches.
    let cors = if state.allowed_origins.is_empty() {
        CorsLayer::new()
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_origin(Any)
    } else {
        let origins: Vec<HeaderValue> = state
            .allowed_origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        CorsLayer::new()
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_origin(AllowOrigin::list(origins))
    };

    let app = Router::new()
        .route("/", post(handle_post))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .map_err(|e| crate::SpallCliError::HttpClient(format!("bind {}: {}", listen_addr, e)))?;
    let bound = listener
        .local_addr()
        .map_err(|e| crate::SpallCliError::HttpClient(format!("local_addr: {}", e)))?;

    // Sentinel line that e2e tests parse to discover the OS-assigned
    // port when `--spall-port 0` is used.
    eprintln!("[spall-mcp] listening on http://{}/", bound);

    axum::serve(listener, app)
        .await
        .map_err(|e| crate::SpallCliError::HttpClient(format!("serve: {}", e)))?;
    Ok(())
}

struct HttpState {
    spec: ResolvedSpec,
    profiles: AuthProfiles,
    registry: IndexMap<String, ToolEntry>,
    sessions: RwLock<HashSet<String>>,
    allowed_origins: HashSet<String>,
}

async fn handle_post(State(state): State<Arc<HttpState>>, headers: HeaderMap, body: String) -> Response {
    // Origin policy:
    // - Empty allowlist + Origin absent → allow (curl, MCP test
    //   clients, same-process). Most browsers always send Origin on
    //   POST; missing Origin implies non-browser caller.
    // - Empty allowlist + Origin localhost → allow (page running on
    //   the same host).
    // - Empty allowlist + Origin remote → REJECT. This is the DNS
    //   rebinding mitigation the MCP spec requires. Previously we
    //   skipped the check entirely when the allowlist was empty,
    //   which left the localhost-default deployment open to remote
    //   attackers who control DNS for `localhost.example.com`.
    // - Non-empty allowlist → require exact Origin match.
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let allowed = if state.allowed_origins.is_empty() {
        origin.is_empty() || is_localhost_origin(origin)
    } else {
        state.allowed_origins.contains(origin)
    };
    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            format!(
                "origin '{}' not allowed (configure with --spall-allowed-origin)",
                origin
            ),
        )
            .into_response();
    }

    // Peek the JSON-RPC method to gate session-id requirements.
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            // Per spec, malformed JSON-RPC requests are 400 with an
            // RPC parse error envelope. Build the envelope inline so
            // the caller can still parse the response.
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": "Parse error" },
                })),
            )
                .into_response();
        }
    };
    let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");

    let is_initialize = method == "initialize";

    // Session-id gate: required on everything except `initialize`.
    // `notifications/initialized` and `ping` need a valid session so
    // a client can't bypass the handshake by sending a noop first.
    if !is_initialize {
        let sid = headers
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let sessions = state.sessions.read().await;
        if !sessions.contains(sid) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": parsed.get("id").cloned().unwrap_or(Value::Null),
                    "error": {
                        "code": -32600,
                        "message": "missing or invalid Mcp-Session-Id; call `initialize` first",
                    },
                })),
            )
                .into_response();
        }
    }

    // Dispatch via the same line handler the stdio transport uses.
    let response = handle_line(&body, &state.spec, &state.profiles, &state.registry).await;

    let mut headers_out = HeaderMap::new();
    if is_initialize {
        let sid = new_session_id();
        state.sessions.write().await.insert(sid.clone());
        if let Ok(v) = HeaderValue::from_str(&sid) {
            headers_out.insert(
                HeaderName::from_static(HEADER_SESSION_ID),
                v,
            );
        }
    }

    let body = response.unwrap_or(Value::Null);
    (headers_out, Json(body)).into_response()
}

/// True for `Origin` headers pointing at the same machine the default
/// `127.0.0.1` bind serves. Used by the default-allowlist Origin check
/// in [`handle_post`] to ride alongside the spec's DNS-rebinding
/// guidance.
fn is_localhost_origin(origin: &str) -> bool {
    matches!(origin, "http://localhost" | "https://localhost"
        | "http://127.0.0.1" | "https://127.0.0.1"
        | "http://[::1]" | "https://[::1]")
        || origin.starts_with("http://localhost:")
        || origin.starts_with("https://localhost:")
        || origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("https://127.0.0.1:")
        || origin.starts_with("http://[::1]:")
        || origin.starts_with("https://[::1]:")
}

/// Generate a 128-bit random hex session id. `rand::thread_rng()` is
/// already in the dep tree for OAuth2 PKCE; we reuse it for parity
/// with the auth subsystem's RNG source.
fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(32);
    for b in &bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_thirty_two_hex_chars() {
        let sid = new_session_id();
        assert_eq!(sid.len(), 32);
        assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn session_ids_are_unique() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b, "two consecutive ids must differ");
    }
}
