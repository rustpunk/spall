//! Streamable HTTP transport for `spall mcp`.
//!
//! Implements MCP spec 2025-06-18 §HTTP for the server side. Single
//! POST endpoint at `/` accepts JSON-RPC requests and replies with the
//! same JSON-RPC framing the stdio transport uses — the dispatch path
//! is shared via [`super::handle_line`].
//!
//! Wire contract:
//! - **POST /** with `Content-Type: application/json`. Body is one
//!   JSON-RPC 2.0 request frame. A request (a frame with an `id`)
//!   content-negotiates its reply on the `Accept` header: the default
//!   (`application/json` or no `Accept`) is one JSON-RPC reply object;
//!   `Accept: text/event-stream` switches the reply to a `text/event-stream`
//!   body carrying one `data:` event per yielded frame (see *SSE
//!   responses* below). A notification/response frame (no reply) gets
//!   `202 Accepted` with an empty body, per the spec's "Sending
//!   Messages" rule.
//! - **GET /** with `Accept: text/event-stream` opens the server→client
//!   SSE channel (progress, sampling, roots). See *SSE responses*.
//! - **DELETE /** with `Mcp-Session-Id` terminates that session. The
//!   Origin gate applies identically (rejecting before the session-id
//!   is read). A valid header returns `200 OK`; termination is
//!   idempotent — a DELETE for an absent session id still returns
//!   `200 OK` (the goal state is reached either way). A missing or
//!   empty `Mcp-Session-Id` header returns `400 Bad Request`.
//! - **`Mcp-Session-Id`** is issued on `initialize` and required on
//!   every subsequent request. Sessions expire after
//!   [`SESSION_TTL_SECS`] of inactivity; a background task prunes
//!   expired entries. Expired session IDs receive 400 with a
//!   re-initialize hint.
//! - **`MCP-Protocol-Version`** is validated on every post-`initialize`
//!   request. An absent header means a pre-header client, so the server
//!   assumes `2025-03-26` and proceeds; a present-but-unsupported
//!   version (outside [`SUPPORTED_PROTOCOL_VERSIONS`]) receives 400.
//!   `initialize` is exempt — the client has not yet learned a version.
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
//! The MCP spec allows `text/event-stream` both for a POST reply (when
//! the client `Accept`s it) and for a server→client GET channel. spall
//! supports both shapes:
//!
//! - **POST content negotiation.** [`super::handle_line`] is
//!   stream-shaped (`Stream<Item = Value>`), so [`handle_post`] drains
//!   it and chooses the reply framing on `Accept`. The default is
//!   `application/json` carrying the single JSON-RPC reply (the server
//!   MAY always answer JSON, per "Sending Messages" clause 5);
//!   `Accept: text/event-stream` emits one `data:` event per yielded
//!   frame, then the stream closes. Every production method yields 0 or
//!   1 frame today, so the multi-frame SSE path is exercised only by the
//!   `#[cfg(debug_assertions)]` placeholder tool — a real multi-frame
//!   source (tool progress) is issue #48.
//! - **GET channel.** [`handle_get`] serves `GET /` as a keep-alive-only
//!   SSE stream — the conformant "I offer a stream but have nothing to
//!   push yet" shape. There is no server-push source in spall v1, so the
//!   stream emits only the keep-alive comment pings; per-session push
//!   subscription state is issue #47.
//!
//! ### Out of scope (file a new issue if needed)
//!
//! - Progress notifications from a long-running tool source (issue #48).
//! - Per-session server-push subscription state on the GET channel
//!   (issue #47).
//! - TLS termination (reverse-proxy responsibility).
//! - Auth on the HTTP endpoint itself (reverse-proxy responsibility).

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::post,
    Json, Router,
};
use futures::StreamExt;
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

/// Header name carrying the negotiated protocol version on every
/// post-`initialize` request, per MCP spec 2025-06-18 §HTTP
/// "Protocol Version Header".
const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";

/// Protocol versions this server accepts on the
/// [`HEADER_PROTOCOL_VERSION`] header. Includes the advertised
/// version ([`super::PROTOCOL_VERSION`], `2025-06-18`), the version
/// assumed when the header is absent (`2025-03-26`, per the spec's
/// backward-compatibility rule), and the normatively-equivalent
/// successor `2025-11-25` so newer clients are not rejected. When the
/// header is present but not in this set, the request is rejected with
/// `400 Bad Request`.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2025-11-25"];

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
    verbose: bool,
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

    if verbose {
        super::emit_verbose_startup(&api_name, "http", &registry, &profiles);
    }

    let state = Arc::new(HttpState {
        spec,
        profiles,
        registry,
        sessions: RwLock::new(HashMap::new()),
        allowed_origins: allowed_origins.into_iter().collect(),
        verbose,
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
        .route("/", post(handle_post).delete(handle_delete).get(handle_get))
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
    /// `--spall-verbose` flag. Consumed by `handle_post` to emit
    /// `[spall-mcp] kind=http-request ...` lines (commit 2 wires this).
    verbose: bool,
}

/// Shared Origin gate for every HTTP method on `/`. Returns
/// `Some(403 response)` when the request's `Origin` is not allowed and
/// `None` when it passes, applying the DNS-rebinding policy described on
/// [`HttpState`]. POST, DELETE, and any future verb call this first so
/// the rejection logic — and the `kind=http-request` verbose line — are
/// identical across methods. (Returning `Option` rather than `Result`
/// keeps the large `Response` off a `Result::Err` variant, which the
/// `result_large_err` lint flags.)
fn check_origin(headers: &HeaderMap, state: &HttpState) -> Option<Response> {
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

    if state.verbose {
        let origin_str = if origin.is_empty() {
            "<absent>".to_string()
        } else {
            origin.to_string()
        };
        let outcome = if state.allowed_origins.is_empty() {
            if allowed {
                "<any>".to_string()
            } else {
                "rejected:remote-origin-with-empty-allowlist".to_string()
            }
        } else if allowed {
            origin.to_string()
        } else {
            "rejected:not-in-allowlist".to_string()
        };
        eprintln!(
            "{} kind=http-request origin={} allowlist={} headers={}",
            super::verbose::SENTINEL,
            super::verbose::quote_if_needed(&origin_str),
            super::verbose::quote_if_needed(&outcome),
            super::verbose::format_headers(headers),
        );
    }

    if allowed {
        None
    } else {
        Some(
            (
                StatusCode::FORBIDDEN,
                format!(
                    "origin '{}' not allowed (configure with --spall-allowed-origin)",
                    origin
                ),
            )
                .into_response(),
        )
    }
}

/// Shared session-id gate. Reads [`HEADER_SESSION_ID`], checks it
/// against the live session map, and refreshes the last-seen marker on
/// a hit. Returns `Some(400 response)` for a missing, empty, expired, or
/// unknown session id (carrying the supplied JSON-RPC `id` in the error
/// envelope) and `None` on a fresh session. POST uses this to gate
/// post-`initialize` requests; future verbs that require an established
/// session reuse the identical logic. (`Option` rather than `Result`
/// mirrors [`check_origin`] and keeps the large `Response` off a
/// `Result::Err` variant.)
async fn check_session_id(
    headers: &HeaderMap,
    state: &HttpState,
    rpc_id: &Value,
) -> Option<Response> {
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
    if valid {
        None
    } else {
        Some(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id.clone(),
                    "error": {
                        "code": -32600,
                        "message": "missing, expired, or invalid Mcp-Session-Id; call `initialize` first",
                    },
                })),
            )
                .into_response(),
        )
    }
}

async fn handle_post(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Some(resp) = check_origin(&headers, &state) {
        return resp;
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
        let rpc_id = parsed.get("id").cloned().unwrap_or(Value::Null);
        if let Some(resp) = check_session_id(&headers, &state, &rpc_id).await {
            return resp;
        }

        // MCP-Protocol-Version gate, applied after the session check so
        // an invalid session still wins (it is the more fundamental
        // gate). Per the spec's backward-compatibility rule: an ABSENT
        // header means the client predates the header convention, so we
        // assume `2025-03-26` and proceed. A PRESENT but unsupported
        // version is rejected with 400. `initialize` is exempt — the
        // client has not yet learned the version to send.
        if let Some(version) = headers
            .get(HEADER_PROTOCOL_VERSION)
            .and_then(|v| v.to_str().ok())
        {
            if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": parsed.get("id").cloned().unwrap_or(Value::Null),
                        "error": {
                            "code": -32600,
                            "message": format!(
                                "unsupported MCP-Protocol-Version '{}'; supported: {}",
                                version,
                                SUPPORTED_PROTOCOL_VERSIONS.join(", "),
                            ),
                        },
                    })),
                )
                    .into_response();
            }
        }
    }

    // Dispatch via the same line handler the stdio transport uses, then
    // drain the reply stream into an owned vector. Every production
    // method yields 0 frames (notifications) or exactly 1 frame
    // (`initialize`, `ping`, `tools/list`, `tools/call`, parse errors);
    // only the `#[cfg(debug_assertions)]` multi-frame placeholder yields >1.
    let frames: Vec<Value> = handle_line(
        &body,
        &state.spec,
        &state.profiles,
        &state.registry,
        state.verbose,
    )
    .await
    .collect()
    .await;

    // Mint the session id for `initialize` so the negotiated id rides on
    // whichever response shape (JSON or SSE) we choose below.
    let mut session_header: Option<(HeaderName, HeaderValue)> = None;
    if is_initialize {
        let sid = new_session_id();
        state
            .sessions
            .write()
            .await
            .insert(sid.clone(), Instant::now());
        if let Ok(v) = HeaderValue::from_str(&sid) {
            session_header = Some((HeaderName::from_static(HEADER_SESSION_ID), v));
        }
    }

    // A POST whose body is only notifications/responses carries no reply
    // frame: per MCP spec 2025-06-18 §HTTP "Sending Messages" clause 4
    // the server MUST answer `202 Accepted` with no body. `handle_line`
    // yields an empty stream for exactly those inputs
    // (`notifications/initialized`, `notifications/cancelled`, and
    // unknown-method notifications), so `frames.is_empty()` is the
    // precise "no reply frame" signal. INVARIANT: every method that
    // carries an `id` yields ≥1 frame from `handle_line` — none yields
    // zero — so this predicate never mis-classifies a real request as a
    // notification. Reachable only for the non-`initialize` path:
    // `initialize` always yields a frame, so the session-mint above runs
    // before this branch can fire (and `initialize` is never a
    // notification, so a 202 here never drops a freshly-minted id).
    if frames.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }

    // Content-negotiate the reply shape on the request `Accept` header.
    // Per MCP 2025-06-18 §HTTP "Sending Messages" clause 5 the server MAY
    // always answer with `application/json`; it MUST answer with
    // `text/event-stream` only when the client asked for it. A client
    // opts into SSE by listing `text/event-stream` in `Accept`.
    let wants_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|a| a.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);

    if wants_sse {
        // One SSE `Event` per reply frame, then the stream closes
        // (`stream::iter` completes after the last frame). Each frame is
        // already a JSON-RPC `Value`; `Event::json_data` serializes it
        // onto the `data:` field. Serialization of a `serde_json::Value`
        // cannot fail in practice, so a failure degrades to an empty SSE
        // comment rather than tearing down the response.
        let events = frames.into_iter().map(|frame| {
            let event = Event::default()
                .json_data(&frame)
                .unwrap_or_else(|_| Event::default().comment("frame serialization failed"));
            Ok::<Event, std::convert::Infallible>(event)
        });
        let sse = Sse::new(futures::stream::iter(events));
        return match session_header {
            Some(header) => ([header], sse).into_response(),
            None => sse.into_response(),
        };
    }

    // Default JSON path. Real dispatch is single-frame, so send that
    // frame as the response body. The only >1-frame producer is the
    // `#[cfg(debug_assertions)]` multi-event placeholder; when a JSON reply is
    // demanded for it, answer with the final (result) frame — the
    // notification frames ahead of it have no JSON-single-response slot.
    let body = match frames.len() {
        1 => frames.into_iter().next().unwrap_or(Value::Null),
        _ => frames.into_iter().last().unwrap_or(Value::Null),
    };
    let mut headers_out = HeaderMap::new();
    if let Some((name, value)) = session_header {
        headers_out.insert(name, value);
    }
    (headers_out, Json(body)).into_response()
}

/// Server-initiated SSE channel, per MCP spec 2025-06-18 §HTTP. A client
/// opens `GET /` (with `Accept: text/event-stream`) to receive
/// server→client messages (progress, sampling, roots). spall v1 has no
/// server-push source yet, so this returns a keep-alive-only stream:
/// the connection stays open and emits SSE comment pings, the conformant
/// "I offer a stream but have nothing to send yet" shape.
///
/// The Origin and session-id gates run identically to `handle_post` /
/// `handle_delete` via the shared [`check_origin`] / [`check_session_id`]
/// helpers, so the GET channel cannot bypass the handshake or the
/// DNS-rebinding policy.
///
/// The per-session push source is tracked in issue #47: when a tool
/// gains a long-running progress source, this handler would subscribe to
/// that session's channel instead of `stream::pending`.
async fn handle_get(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    if let Some(resp) = check_origin(&headers, &state) {
        return resp;
    }

    // GET carries no JSON-RPC body, so the session gate uses a null id in
    // any error envelope (mirroring the DELETE missing-id path).
    if let Some(resp) = check_session_id(&headers, &state, &Value::Null).await {
        return resp;
    }

    // Keep-alive-only: `stream::pending` never yields a data frame, so
    // the only bytes on the wire are the periodic SSE comment pings the
    // keep-alive driver emits. See issue #47 for the future per-session
    // push source that would replace this pending stream.
    let stream = futures::stream::pending::<Result<Event, std::convert::Infallible>>();
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Client-initiated session termination, per MCP spec 2025-06-18 §HTTP
/// "Session Management": `DELETE /` with `Mcp-Session-Id` ends that
/// session.
///
/// - The [`check_origin`] gate runs first and rejects (403) before the
///   session-id is read, identically to `handle_post`.
/// - A missing or empty `Mcp-Session-Id` header is a malformed request:
///   `400 Bad Request` with the standard JSON-RPC error envelope.
/// - Otherwise the session is removed and `200 OK` returned with no
///   body. Termination is IDEMPOTENT: removing an id that is absent
///   (already terminated, expired, or never issued) still returns
///   `200 OK`, because the goal state — "this session no longer
///   exists" — holds either way. Mid-flight requests on a terminated
///   session are out of scope; the existing `RwLock` on the session map
///   is the only synchronization needed.
async fn handle_delete(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    if let Some(resp) = check_origin(&headers, &state) {
        return resp;
    }

    let sid = headers
        .get(HEADER_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if sid.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32600,
                    "message": "missing or empty Mcp-Session-Id; DELETE requires the session id to terminate",
                },
            })),
        )
            .into_response();
    }

    // `remove` returning `None` (absent id) is fine — termination is
    // idempotent, so a second DELETE for the same id still reaches the
    // 200 OK goal state.
    state.sessions.write().await.remove(sid);
    StatusCode::OK.into_response()
}

/// True for `Origin` headers pointing at the same machine the default
/// `127.0.0.1` bind serves. Used by the default-allowlist Origin check
/// in [`handle_post`] to ride alongside the spec's DNS-rebinding
/// guidance.
fn is_localhost_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://localhost"
            | "https://localhost"
            | "http://127.0.0.1"
            | "https://127.0.0.1"
            | "http://[::1]"
            | "https://[::1]"
    ) || origin.starts_with("http://localhost:")
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
