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
//!   `application/json`.
//! - **`Mcp-Session-Id`** is issued on `initialize` and required on
//!   every subsequent request. Sessions expire after
//!   [`SESSION_TTL_SECS`] of inactivity; a background task prunes
//!   expired entries. Expired session IDs receive 400 with a
//!   re-initialize hint.
//! - **Origin** validation: when `--spall-allowed-origin <origin>` is
//!   provided (repeatable), only listed origins succeed. With an
//!   empty allowlist (the default), localhost Origins and requests
//!   without an Origin header succeed; remote Origins receive 403.
//!   Either path rejects before deserializing the body, mitigating
//!   DNS rebinding.
//! - **Bind interface** defaults to `127.0.0.1`. The spec recommends
//!   localhost-only by default; `--spall-bind <addr>` opts into
//!   exposing the server on other interfaces.
//! - **Body size** capped at [`MAX_BODY_BYTES`] (16 MiB) via axum's
//!   `DefaultBodyLimit`; larger requests yield 413.
//!
//! ### SSE responses
//!
//! The MCP spec allows `text/event-stream` for long-running tool
//! responses and for server→client notifications (progress, sampling,
//! roots). spall v1 returns JSON for every reply: tools are all
//! request/response and there is no GET event channel.
//!
//! Adding SSE later is **not** a content-type flip in this handler —
//! [`super::handle_line`] returns `Option<Value>` synchronously, so
//! streaming requires changing the dispatcher's signature to be
//! stream-shaped (`Stream<Item = Value>`), threading progress
//! callbacks through `handle_tools_call`, and adding a GET route for
//! the server-pump channel. That is a transport refactor, tracked
//! separately.
//!
//! ### Out of scope (file a new issue if needed)
//!
//! - SSE for long-running tools / progress notifications.
//! - Server-initiated GET event stream.
//! - TLS termination (reverse-proxy responsibility).
//! - Auth on the HTTP endpoint itself (reverse-proxy responsibility).

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
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use super::{handle_line, AuthProfiles, ToolEntry};

/// Cap on request body size. axum's default is 2 MiB which is too
/// small for OpenAPI specs that legitimately have multipart uploads;
/// 16 MiB covers the common cases without inviting OOM via large
/// adversarial payloads. The matching configuration lives in
/// `axum::extract::DefaultBodyLimit`.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Idle timeout for an MCP HTTP session. A session whose
/// `Mcp-Session-Id` hasn't been seen for this long is expired:
/// further requests carrying it return `400 Bad Request` with the
/// re-initialize hint, and the background pruner reclaims its slot
/// in the session map. One hour matches the FastMCP / Inspector
/// convention and is long enough for any practical agent session.
pub const SESSION_TTL_SECS: u64 = 3600;

/// How often the background task scans the session map for expired
/// entries. Set to TTL / 12 so the worst-case latency between
/// expiry and reclamation is ~5 minutes — well within the headroom
/// of a 1-hour TTL.
const PRUNE_INTERVAL_SECS: u64 = SESSION_TTL_SECS / 12;

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
        sessions: RwLock::new(HashMap::new()),
        allowed_origins: allowed_origins.into_iter().collect(),
    });

    // Spawn the idle-session pruner. Holds a clone of the Arc so the
    // task lives as long as the server. With the current_thread tokio
    // runtime, this co-schedules with handle_post; the await on
    // sleep() always yields, so the pruner can't starve requests.
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let interval = Duration::from_secs(PRUNE_INTERVAL_SECS);
            let ttl = Duration::from_secs(SESSION_TTL_SECS);
            loop {
                tokio::time::sleep(interval).await;
                let now = Instant::now();
                let mut sessions = state.sessions.write().await;
                sessions.retain(|_, last_seen| now.duration_since(*last_seen) < ttl);
            }
        });
    }

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
    /// Map session-id → instant of last activity. The session-id
    /// gate in `handle_post` checks both presence AND freshness against
    /// `SESSION_TTL_SECS`; the background pruner reclaims expired
    /// slots periodically.
    sessions: RwLock<HashMap<String, Instant>>,
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
    // `notifications/initialized` and `ping` need a valid session so a
    // client can't bypass the handshake. Expired sessions (idle past
    // SESSION_TTL_SECS) get the same 400 with a re-init hint so the
    // client can recover without restarting.
    if !is_initialize {
        let sid = headers
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let ttl = Duration::from_secs(SESSION_TTL_SECS);
        let now = Instant::now();
        let valid = {
            let mut sessions = state.sessions.write().await;
            match sessions.get(&sid) {
                Some(last_seen) if now.duration_since(*last_seen) < ttl => {
                    // Bump the last-seen marker so an active session
                    // doesn't expire mid-flight.
                    sessions.insert(sid.clone(), now);
                    true
                }
                Some(_) => {
                    // Expired — reclaim immediately so the next probe
                    // sees a clean miss.
                    sessions.remove(&sid);
                    false
                }
                None => false,
            }
        };
        if !valid {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": parsed.get("id").cloned().unwrap_or(Value::Null),
                    "error": {
                        "code": -32600,
                        "message": "missing, expired, or invalid Mcp-Session-Id; call `initialize` first",
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
        state.sessions.write().await.insert(sid.clone(), Instant::now());
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
