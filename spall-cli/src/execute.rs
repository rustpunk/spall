use crate::matches::MergedMatches;
use clap::ArgMatches;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, COOKIE};
use spall_config::registry::ApiEntry;
use spall_core::ir::{
    HttpMethod, ParameterLocation, ResolvedOperation, ResolvedRequestBody, ResolvedSpec,
};

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::Instant;

static LAST_RESPONSE: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Store the last successful response JSON for pipe/chain consumers.
pub fn store_last_response(value: serde_json::Value) {
    if let Ok(mut guard) = LAST_RESPONSE.lock() {
        *guard = Some(value);
    }
}

/// Take (consume) the last stored response JSON.
pub fn take_last_response() -> Option<serde_json::Value> {
    LAST_RESPONSE.lock().ok().and_then(|mut g| g.take())
}

/// Collect remaining arguments after Phase 1 match.
pub fn collect_remaining_args(matches: &ArgMatches) -> Vec<String> {
    if let Some((ext_name, ext_matches)) = matches.subcommand() {
        let mut remaining = vec![ext_name.to_string()];
        if let Some(vals) = ext_matches.get_many::<std::ffi::OsString>("") {
            for v in vals {
                remaining.push(v.to_string_lossy().to_string());
            }
        }
        remaining
    } else {
        vec!["--help".to_string()]
    }
}

/// Structured return value from `execute_operation_programmatic` and the
/// clap-driven [`execute_operation`].
///
/// `headers` are stored with **lowercased** names so callers can do
/// case-insensitive lookups per RFC 9110 without re-walking. Both 2xx
/// and non-success responses populate this struct; callers that want
/// 4xx/5xx surfaced as an error invoke [`raise_for_status`].
#[derive(Debug)]
pub struct OperationResult {
    pub status: reqwest::StatusCode,
    pub value: serde_json::Value,
    pub headers: BTreeMap<String, String>,
}

/// Structured arguments for `execute_operation_programmatic`.
///
/// All fields are owned `String` / `Value`s; no `ArgMatches` lifetimes
/// leak through. `#[non_exhaustive]` keeps future field additions
/// non-breaking for downstream callers (the Arazzo runner, future MCP
/// dispatcher, REPL).
#[derive(Debug)]
#[non_exhaustive]
pub struct ProgrammaticArgs {
    /// Path parameters keyed by the parameter `name` from the spec.
    pub path: BTreeMap<String, String>,
    /// Query parameters keyed by name. One value per name; for
    /// array-valued query params with `explode = true`, populate
    /// [`Self::query_extras`] instead and reqwest will emit
    /// `?ids=1&ids=2` per OpenAPI `style: form` semantics.
    pub query: BTreeMap<String, String>,
    /// Additional query pairs appended *after* [`Self::query`]. Used by
    /// the MCP dispatcher to expand array arguments per OpenAPI
    /// `style: form, explode: true` (the default for query parameters in
    /// OpenAPI 3.x). Same key may appear multiple times — reqwest's
    /// query serializer preserves the repetition.
    pub query_extras: Vec<(String, String)>,
    /// Headers keyed by canonical-case name. Applied **after** auth
    /// resolution, so callers may override an `Authorization` header set
    /// by spall's auth chain.
    pub header: BTreeMap<String, String>,
    /// Cookie parameters keyed by name.
    pub cookie: BTreeMap<String, String>,
    /// Optional request body. Serialized as JSON; `Content-Type:
    /// application/json` is set unless the caller already supplied a
    /// `Content-Type` header.
    pub body: Option<serde_json::Value>,
    /// CLI-style `--spall-auth` override (e.g. `Bearer xxx`), forwarded
    /// to `crate::auth::resolve` as the highest-priority source.
    pub auth_override: Option<String>,
    /// If `Some`, this server URL overrides the one resolved from
    /// `ApiEntry::base_url` / operation / spec.
    pub server_override: Option<String>,
    /// HTTP client configuration.
    pub http: crate::http::HttpConfig,
    /// Retry attempts beyond the first. Default `1`.
    pub retry_count: u8,
    /// Cap on `Retry-After` delay in seconds. Default `60`.
    pub retry_max_wait_secs: u64,
}

impl Default for ProgrammaticArgs {
    /// Empty maps and `None`s, with sensible retry defaults
    /// (`retry_count = 1`, `retry_max_wait_secs = 60`). Picking these as
    /// defaults — rather than the zero values a `#[derive(Default)]`
    /// would yield — means non-CLI callers (Arazzo runner, future MCP
    /// server) get the same retry behavior as a bare `spall <api> <op>`
    /// invocation without having to spell it out.
    fn default() -> Self {
        Self {
            path: BTreeMap::new(),
            query: BTreeMap::new(),
            query_extras: Vec::new(),
            header: BTreeMap::new(),
            cookie: BTreeMap::new(),
            body: None,
            auth_override: None,
            server_override: None,
            http: crate::http::HttpConfig::default(),
            retry_count: 1,
            retry_max_wait_secs: 60,
        }
    }
}

impl ProgrammaticArgs {
    /// Alias for [`Default::default`]. Kept for historical call sites;
    /// new code should prefer `ProgrammaticArgs::default()`.
    #[must_use = "the constructed args are the only output"]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Convert a non-success [`OperationResult`] into the matching
/// `SpallCliError` HTTP variant. 2xx passes through unchanged.
///
/// Both the clap and programmatic execution paths return `OperationResult`
/// for any HTTP status; 4xx / 5xx → `Err` is left to the caller so that
/// response headers and body are still observable on failure (history
/// recording, MCP error payloads, Arazzo step-failure hints).
#[must_use = "ignoring this Result swallows HTTP failure status"]
pub fn raise_for_status(res: OperationResult) -> Result<OperationResult, crate::SpallCliError> {
    let code = res.status.as_u16();
    if res.status.is_client_error() {
        return Err(crate::SpallCliError::Http4xx(code));
    }
    if res.status.is_server_error() {
        return Err(crate::SpallCliError::Http5xx(code));
    }
    Ok(res)
}

/// Programmatic entry point into spall's request pipeline.
///
/// This is the *canonical* execution path for callers that do not have
/// `clap::ArgMatches` available — the Arazzo workflow runner, future MCP
/// server, embedded REPL drivers. The clap-driven [`execute_operation`]
/// delegates its single-shot path through the shared
/// [`prepare_and_send`] helper, guaranteeing both paths produce identical
/// outbound requests for the same inputs.
///
/// What this **does**: URL build, header / query / cookie / path / auth
/// resolution, JSON body serialization, a single retrying `send_one`,
/// body JSON parse. Returns `OperationResult` for *any* HTTP status —
/// callers that want 4xx/5xx surfaced as errors invoke
/// [`raise_for_status`].
///
/// What this **does NOT do** (those belong in the clap wrapper):
/// dry-run, preview, preflight validation, pagination, hypermedia
/// follow, history recording, `store_last_response`, verbose/time
/// stderr logging, response-schema warning emission.
#[must_use = "the OperationResult carries the response body"]
pub async fn execute_operation_programmatic(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    args: &ProgrammaticArgs,
) -> Result<OperationResult, crate::SpallCliError> {
    let (status, resp_hdrs, body_bytes) = prepare_and_send(op, spec, entry, args).await?;
    let value = serde_json::from_slice::<serde_json::Value>(&body_bytes).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&body_bytes).to_string())
    });
    Ok(OperationResult {
        status,
        value,
        headers: lowercase_headers(&resp_hdrs),
    })
}

/// Shared request-assembly + send pipeline.
///
/// Both the programmatic entry point above and the clap-driven
/// `execute_operation`'s single-shot branch call this. Centralizing the
/// "build URL → headers → cookies → query → auth → caller headers →
/// body → send" sequence here is what prevents drift between the two
/// callers (the parity guard test in
/// `spall-cli/tests/execute_parity_test.rs` exercises this).
pub(crate) async fn prepare_and_send(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    args: &ProgrammaticArgs,
) -> Result<(reqwest::StatusCode, HeaderMap, Vec<u8>), crate::SpallCliError> {
    let url = build_url_with_path_args(op, spec, entry, args.server_override.as_deref(), &args.path);

    let mut headers = HeaderMap::new();

    // Step 1: default headers from the ApiEntry config.
    for (k, v) in &entry.default_headers {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            headers.insert(name, value);
        }
    }

    // Step 2: cookies → COOKIE header.
    if !args.cookie.is_empty() {
        let joined = args
            .cookie
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");
        if let Ok(v) = HeaderValue::from_str(&joined) {
            headers.insert(COOKIE, v);
        }
    }

    // Step 3: query pairs. The dedup'd map yields one pair per key;
    // `query_extras` adds any further pairs (repeated keys allowed) for
    // OpenAPI `style: form, explode: true` array serialization.
    let mut query_pairs: Vec<(String, String)> = args
        .query
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    query_pairs.extend(args.query_extras.iter().cloned());

    // Step 4: auth resolution + injection.
    let resolved_auth =
        crate::auth::resolve(&entry.name, entry.auth.as_ref(), args.auth_override.as_deref())
            .await?;
    if let Some(a) = resolved_auth {
        crate::auth::apply(&a, &mut headers, &mut query_pairs);
    }

    // Step 5: caller-supplied headers win over the auth chain.
    for (k, v) in &args.header {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            headers.insert(name, value);
        }
    }

    // Step 6: body serialization.
    let body_bytes: Option<Vec<u8>> = if let Some(value) = &args.body {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| crate::SpallCliError::Usage(format!("invalid JSON body: {}", e)))?;
        if !headers.contains_key(reqwest::header::CONTENT_TYPE) {
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        Some(bytes)
    } else {
        None
    };

    // Step 7: build client + send.
    let client =
        crate::http::build_http_client(&args.http).map_err(crate::SpallCliError::HttpClient)?;
    send_one(
        &client,
        op.method,
        &url,
        headers,
        body_bytes,
        None,
        &query_pairs,
        args.retry_count,
        args.retry_max_wait_secs,
    )
    .await
}

fn lowercase_headers(h: &HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, value) in h.iter() {
        if let Ok(s) = value.to_str() {
            out.insert(name.as_str().to_ascii_lowercase(), s.to_string());
        }
    }
    out
}

/// URL builder shared by the clap-driven and programmatic paths.
///
/// `server_override`, when `Some`, supersedes `entry.base_url` and the
/// per-op / per-spec server lists. `path_args` is consulted for
/// path-template substitution. `pub(crate)` so the Arazzo runner can
/// reuse it for dry-run URL formatting (no double-slash bugs from
/// hand-formatted strings).
#[must_use = "the assembled URL is the only output"]
pub(crate) fn build_url_with_path_args(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    server_override: Option<&str>,
    path_args: &BTreeMap<String, String>,
) -> String {
    let base = server_override
        .map(|s| s.to_string())
        .or_else(|| entry.base_url.clone())
        .or_else(|| op.servers.first().map(|s| s.url.clone()))
        .or_else(|| spec.servers.first().map(|s| s.url.clone()))
        .unwrap_or_else(|| "/".to_string());

    let mut path = op.path_template.clone();
    for param in &op.parameters {
        if param.location == ParameterLocation::Path {
            if let Some(v) = path_args.get(&param.name) {
                path = path.replace(&format!("{{{}}}", param.name), v);
                path = path.replace(&format!("{{{}*}}", param.name), v);
            }
        }
    }

    let base_trimmed = base.trim_end_matches('/');
    let path_trimmed = if path.starts_with('/') {
        path
    } else {
        format!("/{}", path)
    };
    format!("{}{}", base_trimmed, path_trimmed)
}

/// Execute a matched operation from clap arguments.
///
/// This is a *thin adapter* on top of [`execute_operation_programmatic`]:
/// it translates `ArgMatches` into `ProgrammaticArgs`, then layers
/// CLI-only behavior (preflight validation, dry-run, preview,
/// pagination, hypermedia follow, history recording, verbose/time
/// stderr logging, response-schema warnings, `store_last_response`)
/// around the shared [`prepare_and_send`] pipeline. The non-paginate,
/// non-multipart, non-form single-shot path delegates directly to
/// `execute_operation_programmatic`, which is the canonical pipeline
/// the Arazzo runner and future MCP server also use.
pub async fn execute_operation(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    phase2_matches: &ArgMatches,
    phase1_matches: &ArgMatches,
    cache_dir: &std::path::Path,
    defaults: &spall_config::sources::GlobalDefaults,
) -> Result<OperationResult, crate::SpallCliError> {
    let combined = merge_matches(phase1_matches, phase2_matches);

    // Preflight validation (Wave 2) — clap-only; missing/typed-wrong args
    // are reported with rich diagnostics and short-circuit the request.
    if let Err(errors) = crate::validate::preflight_validate(op, phase2_matches) {
        eprintln!("Validation failed:");
        eprintln!("{}", crate::validate::format_errors(&errors));
        return Err(crate::SpallCliError::ValidationFailed);
    }

    // Resolve the body via the clap-only --data / --form / --field paths.
    // Multipart and form-urlencoded keep flowing through send_one directly
    // because they don't fit ProgrammaticArgs.body (which is JSON).
    let body_data = resolve_body(op.request_body.as_ref(), phase2_matches)?;

    // Build ProgrammaticArgs from clap matches. This is the *only* place
    // the clap → programmatic translation happens; the parity test in
    // tests/execute_parity_test.rs locks down the wire-level behavior.
    let mut http_config = crate::http::config_from_matches(phase1_matches, phase2_matches);
    http_config.proxy = crate::http::resolve_proxy(entry, defaults, phase1_matches, phase2_matches);

    let mut args = args_from_matches(op, phase1_matches, phase2_matches, &combined, http_config);

    // Body translation: --data with a JSON payload + the operation's
    // resolved JSON content type → ProgrammaticArgs.body. Multipart and
    // form-urlencoded fall back to the legacy direct-send_one path below.
    let mut json_body_consumed = false;
    if body_data.multipart.is_none() {
        if let Some(bytes) = body_data.body.as_deref() {
            // If --spall-content-type was supplied, set it as a header so
            // the programmatic path doesn't override it with
            // application/json.
            if let Some(ct) = body_data.content_type.as_deref() {
                args.header.insert("Content-Type".to_string(), ct.to_string());
            }
            // Try to parse the bytes as JSON; if successful, use the
            // structured body. Otherwise (e.g. a raw text payload), fall
            // through to the direct-send_one path so we don't double-encode.
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
                args.body = Some(value);
                json_body_consumed = true;
            }
        }
    }

    let url = build_url(op, spec, entry, phase1_matches, phase2_matches)?;

    // Dry run — clap-only, prints to stderr and returns Null.
    if combined.get_flag("spall-dry-run") {
        eprintln!("Dry run: {} {}", op.method, url);
        store_last_response(serde_json::Value::Null);
        return Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: serde_json::Value::Null,
            headers: BTreeMap::new(),
        });
    }

    // Preview (Phase D) — clap-only.
    if combined.get_flag("spall-preview") {
        // Build a HeaderMap snapshot for preview purposes only.
        let mut preview_headers = HeaderMap::new();
        for (k, v) in &args.header {
            if let (Ok(name), Ok(value)) =
                (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
            {
                preview_headers.insert(name, value);
            }
        }
        let body_slice = body_data.body.as_deref();
        crate::preview::print_preview(&op.method.to_string(), &url, &preview_headers, body_slice);
        store_last_response(serde_json::Value::Null);
        return Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: serde_json::Value::Null,
            headers: BTreeMap::new(),
        });
    }

    // Pagination, multipart, and the form-urlencoded path that didn't
    // round-trip as JSON keep using the legacy direct-send_one flow.
    // These don't fit the ProgrammaticArgs.body shape (multipart needs a
    // streaming Form; form bodies are pre-encoded bytes).
    let needs_legacy = combined.get_flag("spall-paginate")
        || body_data.multipart.is_some()
        || (body_data.body.is_some() && !json_body_consumed);

    if needs_legacy {
        return execute_legacy_path(
            op,
            spec,
            entry,
            phase1_matches,
            phase2_matches,
            cache_dir,
            defaults,
            body_data,
            url,
        )
        .await;
    }

    // Single-shot delegation to the programmatic path — the parity guard.
    let start = Instant::now();
    let result = execute_operation_programmatic(op, spec, entry, &args).await?;

    // Reconstruct a HeaderMap for history-record + response-validate.
    // (Cheap; the lowercased BTreeMap was the cross-process boundary.)
    let mut resp_hdrs = HeaderMap::new();
    for (k, v) in &result.headers {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            resp_hdrs.insert(name, value);
        }
    }

    // Reconstruct a request-side HeaderMap for the history record.
    let mut req_hdrs_for_history = HeaderMap::new();
    for (k, v) in &args.header {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            req_hdrs_for_history.insert(name, value);
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    record_history(
        cache_dir,
        &entry.name,
        &op.operation_id,
        &op.method.to_string(),
        &url,
        Some(result.status),
        &req_hdrs_for_history,
        &resp_hdrs,
        duration_ms,
    );

    if combined.get_flag("spall-verbose") {
        eprintln!("HTTP {} {}", result.status, url);
    }

    // Response validation (warn-only).
    if result.status.is_success() {
        let ct = result
            .headers
            .get("content-type")
            .map(|s| s.as_str())
            .unwrap_or("application/json");
        let warnings =
            crate::validate::response_validate(op, result.status.as_u16(), ct, &result.value);
        if !warnings.is_empty() {
            eprintln!("Warning: response body did not match schema:");
            eprintln!("{}", crate::validate::format_errors(&warnings));
        }
    }

    let raised = raise_for_status(result)?;
    let mut final_value = raised.value;
    let resp_headers_lc = raised.headers;
    let status = raised.status;

    // --spall-follow <rel>: chase one hypermedia link.
    //
    // The followed URL is absolute (per Link headers and HAL `_links`)
    // so URL assembly and path-template substitution from the original
    // operation don't apply. We hit `send_one` directly with a fresh
    // client, reusing the same auth/header set as the primary request.
    if let Some(rel) = combined.get_one::<String>("spall-follow") {
        let body_ref = if final_value.is_object() || final_value.is_array() {
            Some(&final_value)
        } else {
            None
        };
        let links = crate::links::Links::from_response(&resp_hdrs, body_ref);
        if let Some(link) = links.rel(rel.as_str()) {
            let followed_url = resolve_next_url(&url, &link.href)?;
            if combined.get_flag("spall-verbose") {
                eprintln!("Following rel=\"{}\" -> {}", rel, followed_url);
            }
            let client = crate::http::build_http_client(&args.http)
                .map_err(crate::SpallCliError::HttpClient)?;
            // Re-resolve auth so the follow inherits Authorization /
            // ApiKey headers from the same chain that built the primary
            // request — keeps behavior identical to the pre-refactor path.
            let mut follow_headers = HeaderMap::new();
            for (k, v) in &args.header {
                if let (Ok(name), Ok(value)) =
                    (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
                {
                    follow_headers.insert(name, value);
                }
            }
            let mut empty_query: Vec<(String, String)> = Vec::new();
            if let Some(a) = crate::auth::resolve(
                &entry.name,
                entry.auth.as_ref(),
                args.auth_override.as_deref(),
            )
            .await?
            {
                crate::auth::apply(&a, &mut follow_headers, &mut empty_query);
            }
            let (fstatus, _fhdrs, fbytes) = send_one(
                &client,
                HttpMethod::Get,
                &followed_url,
                follow_headers,
                None,
                None,
                &empty_query,
                args.retry_count,
                args.retry_max_wait_secs,
            )
            .await?;
            if fstatus.is_client_error() {
                return Err(crate::SpallCliError::Http4xx(fstatus.as_u16()));
            }
            if fstatus.is_server_error() {
                return Err(crate::SpallCliError::Http5xx(fstatus.as_u16()));
            }
            final_value =
                serde_json::from_slice::<serde_json::Value>(&fbytes).unwrap_or_else(|_| {
                    serde_json::Value::String(String::from_utf8_lossy(&fbytes).to_string())
                });
        } else if combined.get_flag("spall-verbose") {
            eprintln!("No link with rel=\"{}\" found in response", rel);
        }
    }

    let duration = start.elapsed();
    if combined.get_flag("spall-time") || combined.get_flag("spall-verbose") {
        eprintln!("Duration: {:?}", duration);
    }

    store_last_response(final_value.clone());
    Ok(OperationResult {
        status,
        value: final_value,
        headers: resp_headers_lc,
    })
}

/// Translate clap matches into [`ProgrammaticArgs`].
///
/// Pulled out so the parity test can construct equivalent inputs from
/// both directions and assert wire-level equivalence.
pub(crate) fn args_from_matches(
    op: &ResolvedOperation,
    phase1_matches: &ArgMatches,
    phase2_matches: &ArgMatches,
    combined: &MergedMatches<'_>,
    http_config: crate::http::HttpConfig,
) -> ProgrammaticArgs {
    let mut args = ProgrammaticArgs {
        http: http_config,
        retry_count: combined.get_one::<u8>("spall-retry").unwrap_or(1),
        retry_max_wait_secs: combined.get_one::<u64>("spall-retry-max-wait").unwrap_or(60),
        ..ProgrammaticArgs::default()
    };

    args.server_override = phase2_matches
        .get_one::<String>("spall-server")
        .cloned()
        .or_else(|| phase1_matches.get_one::<String>("spall-server").cloned());

    args.auth_override = combined.get_one::<String>("spall-auth");

    for param in &op.parameters {
        let id = match param.location {
            ParameterLocation::Path => format!("path-{}", param.name),
            ParameterLocation::Query => format!("query-{}", param.name),
            ParameterLocation::Header => format!("header-{}", param.name),
            ParameterLocation::Cookie => format!("cookie-{}", param.name),
        };
        let Some(v) = phase2_matches.get_one::<String>(&id) else {
            continue;
        };
        let bucket = match param.location {
            ParameterLocation::Path => &mut args.path,
            ParameterLocation::Query => &mut args.query,
            ParameterLocation::Header => &mut args.header,
            ParameterLocation::Cookie => &mut args.cookie,
        };
        bucket.insert(param.name.clone(), v.clone());
    }

    // --spall-header k:v overrides go through args.header so the
    // shared pipeline applies them after the auth chain (caller wins).
    if let Some(values) = combined.get_many::<String>("spall-header") {
        for h in values {
            if let Some((k, v)) = h.split_once(':') {
                args.header.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }

    args
}

/// Legacy direct-send_one path, retained for paginate / multipart /
/// form-urlencoded — these don't translate cleanly to
/// `ProgrammaticArgs.body` (multipart needs a streaming `Form`; raw
/// form bodies are pre-encoded bytes). The single-shot, JSON-body case
/// in `execute_operation` delegates to `execute_operation_programmatic`
/// instead, sharing the [`prepare_and_send`] pipeline.
#[allow(clippy::too_many_arguments)]
async fn execute_legacy_path(
    op: &ResolvedOperation,
    _spec: &ResolvedSpec,
    entry: &ApiEntry,
    phase1_matches: &ArgMatches,
    phase2_matches: &ArgMatches,
    cache_dir: &std::path::Path,
    defaults: &spall_config::sources::GlobalDefaults,
    body_data: BodyData,
    url: String,
) -> Result<OperationResult, crate::SpallCliError> {
    let combined = merge_matches(phase1_matches, phase2_matches);

    let mut headers = HeaderMap::new();

    // Default headers from config
    for (k, v) in &entry.default_headers {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes())
                .unwrap_or_else(|_| HeaderName::from_static("x-unknown")),
            HeaderValue::from_str(v).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
        );
    }

    // Custom headers from --spall-header
    if let Some(values) = combined.get_many::<String>("spall-header") {
        for h in values {
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(
                    HeaderName::from_bytes(k.trim().as_bytes())
                        .unwrap_or_else(|_| HeaderName::from_static("x-unknown")),
                    HeaderValue::from_str(v.trim())
                        .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
                );
            }
        }
    }

    // Cookie params
    let mut cookies: Vec<String> = Vec::new();
    for param in &op.parameters {
        if param.location == ParameterLocation::Cookie {
            let id = format!("cookie-{}", param.name);
            if let Some(v) = phase2_matches.get_one::<String>(&id) {
                cookies.push(format!("{}={}", param.name, v));
            }
        }
    }
    if !cookies.is_empty() {
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&cookies.join("; "))
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
    }

    // Query params
    let mut query_pairs: Vec<(String, String)> = Vec::new();
    for param in &op.parameters {
        if param.location == ParameterLocation::Query {
            let id = format!("query-{}", param.name);
            if let Some(v) = phase2_matches.get_one::<String>(&id) {
                query_pairs.push((param.name.clone(), v.clone()));
            }
        }
    }

    // Authentication
    let cli_auth = combined.get_one::<String>("spall-auth");
    let auth = crate::auth::resolve(&entry.name, entry.auth.as_ref(), cli_auth.as_deref()).await?;
    if let Some(a) = auth {
        crate::auth::apply(&a, &mut headers, &mut query_pairs);
    }

    // Content-Type
    if let Some(ref content_type) = body_data.content_type {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
    }

    let start = Instant::now();
    let mut http_config = crate::http::config_from_matches(phase1_matches, phase2_matches);
    http_config.proxy = crate::http::resolve_proxy(entry, defaults, phase1_matches, phase2_matches);

    let client = crate::http::build_http_client(&http_config)
        .map_err(crate::SpallCliError::HttpClient)?;

    let retry_count = combined.get_one::<u8>("spall-retry").unwrap_or(1);
    let retry_max_wait = combined.get_one::<u64>("spall-retry-max-wait").unwrap_or(60);

    if combined.get_flag("spall-paginate") {
        if body_data.multipart.is_some() {
            return Err(crate::SpallCliError::Usage(
                "Cannot use --spall-paginate with multipart uploads".to_string(),
            ));
        }
        let paginator = crate::paginate::Paginator::default();
        let mut pages: Vec<serde_json::Value> = Vec::new();
        let mut current_url = url.clone();
        let mut first_status: Option<reqwest::StatusCode> = None;

        for page_num in 0..paginator.max_pages {
            let (status, resp_headers, body_bytes) = send_one(
                &client,
                op.method,
                &current_url,
                headers.clone(),
                if page_num == 0 {
                    body_data.body.clone()
                } else {
                    None
                },
                None,
                &query_pairs,
                retry_count,
                retry_max_wait,
            )
            .await?;
            if first_status.is_none() {
                first_status = Some(status);
            }
            if combined.get_flag("spall-verbose") {
                eprintln!("HTTP {} {}", status, current_url);
            }
            if !status.is_success() {
                if status.is_client_error() {
                    return Err(crate::SpallCliError::Http4xx(status.as_u16()));
                }
                return Err(crate::SpallCliError::Http5xx(status.as_u16()));
            }
            let body_json =
                serde_json::from_slice::<serde_json::Value>(&body_bytes).map_err(|e| {
                    crate::SpallCliError::Usage(format!(
                        "Pagination requires JSON responses: {}",
                        e
                    ))
                })?;
            pages.push(body_json);
            if let Some(next) = paginator.next_url(&resp_headers) {
                current_url = resolve_next_url(&current_url, &next)?;
                query_pairs.clear();
            } else {
                break;
            }
        }

        let final_value = paginator.concat_results(pages);
        if let Some(first) = first_status {
            let warnings =
                crate::validate::response_validate(op, first.as_u16(), "application/json", &final_value);
            if !warnings.is_empty() {
                eprintln!("Warning: response body did not match schema:");
                eprintln!("{}", crate::validate::format_errors(&warnings));
            }
        }
        let duration_ms = start.elapsed().as_millis() as u64;
        record_history(
            cache_dir,
            &entry.name,
            &op.operation_id,
            &op.method.to_string(),
            &current_url,
            first_status,
            &headers,
            &HeaderMap::new(),
            duration_ms,
        );
        store_last_response(final_value.clone());
        return Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: final_value,
            headers: BTreeMap::new(),
        });
    }

    // Multipart / form-urlencoded single shot.
    let (status, resp_headers, body_bytes) = send_one(
        &client,
        op.method,
        &url,
        headers.clone(),
        body_data.body,
        body_data.multipart,
        &query_pairs,
        retry_count,
        retry_max_wait,
    )
    .await?;

    let duration_ms = start.elapsed().as_millis() as u64;
    record_history(
        cache_dir,
        &entry.name,
        &op.operation_id,
        &op.method.to_string(),
        &url,
        Some(status),
        &headers,
        &resp_headers,
        duration_ms,
    );

    if combined.get_flag("spall-verbose") {
        eprintln!("HTTP {} {}", status, url);
    }

    if status.is_success() {
        let ct = resp_headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json");
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            let warnings = crate::validate::response_validate(op, status.as_u16(), ct, &json_val);
            if !warnings.is_empty() {
                eprintln!("Warning: response body did not match schema:");
                eprintln!("{}", crate::validate::format_errors(&warnings));
            }
        }
    }

    let body_json =
        serde_json::from_slice::<serde_json::Value>(&body_bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&body_bytes).to_string())
        });

    if status.is_client_error() {
        return Err(crate::SpallCliError::Http4xx(status.as_u16()));
    }
    if status.is_server_error() {
        return Err(crate::SpallCliError::Http5xx(status.as_u16()));
    }

    let duration = start.elapsed();
    if combined.get_flag("spall-time") || combined.get_flag("spall-verbose") {
        eprintln!("Duration: {:?}", duration);
    }
    store_last_response(body_json.clone());
    Ok(OperationResult {
        status,
        value: body_json,
        headers: lowercase_headers(&resp_headers),
    })
}

/// Send a single HTTP request, with transient-error retry.
///
/// On `429 Too Many Requests` or `503 Service Unavailable`, honors the
/// `Retry-After` header (RFC 7231 §7.1.3) — both delta-seconds and HTTP-date
/// forms — clamped by `retry_max_wait_secs`. If the indicated wait exceeds
/// the clamp, the response is returned as-is.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_one(
    client: &reqwest::Client,
    method: HttpMethod,
    url: &str,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    mut multipart: Option<reqwest::multipart::Form>,
    query_pairs: &[(String, String)],
    retry_count: u8,
    retry_max_wait_secs: u64,
) -> Result<(reqwest::StatusCode, HeaderMap, Vec<u8>), crate::SpallCliError> {
    let max_attempts = if multipart.is_some() {
        1
    } else {
        retry_count + 1
    };
    for attempt in 0..max_attempts {
        let mut req_builder = match method {
            HttpMethod::Get => client.get(url),
            HttpMethod::Post => client.post(url),
            HttpMethod::Put => client.put(url),
            HttpMethod::Delete => client.delete(url),
            HttpMethod::Patch => client.patch(url),
            HttpMethod::Head => client.head(url),
            HttpMethod::Options => client.request(reqwest::Method::OPTIONS, url),
            HttpMethod::Trace => client.request(reqwest::Method::TRACE, url),
        };

        req_builder = req_builder.headers(headers.clone());

        if let Some(m) = multipart.take() {
            req_builder = req_builder.multipart(m);
        } else if let Some(ref b) = body {
            req_builder = req_builder.body(b.clone());
        }

        if !query_pairs.is_empty() {
            req_builder = req_builder.query(query_pairs);
        }

        match req_builder.send().await {
            Ok(r) => {
                let status = r.status();
                let hdrs = r.headers().clone();
                let bytes = r
                    .bytes()
                    .await
                    .map_err(|e| crate::SpallCliError::Network(e.to_string()))?
                    .to_vec();

                // Honor Retry-After on rate-limit / unavailable, if we have a retry budget.
                let retryable = status.as_u16() == 429 || status.as_u16() == 503;
                if retryable && attempt + 1 < max_attempts {
                    if let Some(wait) = parse_retry_after(&hdrs) {
                        if wait.as_secs() <= retry_max_wait_secs {
                            tokio::time::sleep(wait).await;
                            continue;
                        }
                        // Wait exceeds clamp — fall through and return the response.
                    }
                }

                return Ok((status, hdrs, bytes));
            }
            Err(e) => {
                if attempt + 1 < max_attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                return Err(crate::SpallCliError::Network(e.to_string()));
            }
        }
    }
    Err(crate::SpallCliError::Network(
        "request failed after retries".to_string(),
    ))
}

/// Parse a `Retry-After` header. Accepts delta-seconds or HTTP-date per RFC 7231 §7.1.3.
fn parse_retry_after(headers: &HeaderMap) -> Option<std::time::Duration> {
    let v = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(std::time::Duration::from_secs(secs));
    }
    // HTTP-date — return delta to the future, or 0 for past dates.
    let target = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let now = chrono::Utc::now();
    let delta = target.with_timezone(&chrono::Utc) - now;
    if delta.num_seconds() <= 0 {
        Some(std::time::Duration::from_secs(0))
    } else {
        Some(std::time::Duration::from_secs(delta.num_seconds() as u64))
    }
}

/// Resolve a `next` URL from a Link header against the current request URL.
fn resolve_next_url(current: &str, next: &str) -> Result<String, crate::SpallCliError> {
    if next.starts_with("http://") || next.starts_with("https://") {
        Ok(next.to_string())
    } else {
        let base = reqwest::Url::parse(current)
            .map_err(|e| crate::SpallCliError::Network(format!("Invalid current URL: {}", e)))?;
        let resolved = base.join(next).map_err(|e| {
            crate::SpallCliError::Network(format!("Invalid next URL '{}': {}", next, e))
        })?;
        Ok(resolved.to_string())
    }
}

/// Print an operation result to stdout, applying filter and output mode.
pub fn print_operation_result(
    res: &OperationResult,
    combined: &MergedMatches,
) -> Result<(), crate::SpallCliError> {
    let mode = determine_output_mode(combined);
    let save_path_owned = combined.get_one::<String>("spall-download");
    let save_path = save_path_owned.as_deref();

    if let Some(filter_expr) = combined.get_one::<String>("spall-filter") {
        match crate::filter::filter_response(&filter_expr, &res.value) {
            Ok(filtered) => crate::output::emit_json_value(&filtered, mode, save_path)
                .map_err(|e| crate::SpallCliError::HttpClient(e.to_string())),
            Err(e) => {
                eprintln!(
                    "Warning: filter failed ({}). Falling back to unfiltered.",
                    e
                );
                crate::output::emit_json_value(&res.value, mode, save_path)
                    .map_err(|e| crate::SpallCliError::HttpClient(e.to_string()))
            }
        }
    } else {
        crate::output::emit_json_value(&res.value, mode, save_path)
            .map_err(|e| crate::SpallCliError::HttpClient(e.to_string()))
    }
}

fn determine_output_mode(combined: &MergedMatches) -> crate::output::OutputMode {
    if combined.get_flag("spall-verbose") {
        crate::output::OutputMode::Raw
    } else if let Some(output) = combined.get_one::<String>("spall-output") {
        crate::output::OutputMode::from_str(&output).unwrap_or_default()
    } else {
        crate::output::OutputMode::default()
    }
}

/// Merge Phase 1 and Phase 2 ArgMatches, preferring Phase 2 for overlapping values.
pub fn merge_matches<'a>(phase1: &'a ArgMatches, phase2: &'a ArgMatches) -> MergedMatches<'a> {
    MergedMatches { phase1, phase2 }
}

/// Build the full request URL from clap matches.
///
/// Thin adapter: extracts path-template args and the optional
/// `--spall-server` override from `ArgMatches`, then delegates to the
/// shared [`build_url_with_path_args`] used by the programmatic path.
fn build_url(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    phase1_matches: &ArgMatches,
    phase2_matches: &ArgMatches,
) -> Result<String, crate::SpallCliError> {
    let server_override = phase2_matches
        .get_one::<String>("spall-server")
        .cloned()
        .or_else(|| phase1_matches.get_one::<String>("spall-server").cloned());

    let mut path_args: BTreeMap<String, String> = BTreeMap::new();
    for param in &op.parameters {
        if param.location == ParameterLocation::Path {
            let id = format!("path-{}", param.name);
            if let Some(v) = phase2_matches.get_one::<String>(&id) {
                path_args.insert(param.name.clone(), v.clone());
            }
        }
    }
    Ok(build_url_with_path_args(
        op,
        spec,
        entry,
        server_override.as_deref(),
        &path_args,
    ))
}

struct BodyData {
    content_type: Option<String>,
    body: Option<Vec<u8>>,
    multipart: Option<reqwest::multipart::Form>,
}

/// Resolve request body from --data, --form, or --field.
fn resolve_body(
    request_body: Option<&ResolvedRequestBody>,
    phase2_matches: &ArgMatches,
) -> Result<BodyData, crate::SpallCliError> {
    // If the operation has no request body definition, nothing to do.
    let Some(body_def) = request_body else {
        return Ok(BodyData {
            content_type: None,
            body: None,
            multipart: None,
        });
    };

    // --no-data is only registered when the body is optional.
    if !body_def.required && phase2_matches.get_flag("no-data") {
        return Ok(BodyData {
            content_type: None,
            body: None,
            multipart: None,
        });
    }

    // --data
    if let Some(values) = phase2_matches.get_many::<String>("data") {
        let parts: Vec<String> = values.cloned().collect();
        if let Some(last) = parts.last() {
            let data = if last == "-" {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                    crate::SpallCliError::Usage(format!("Failed to read stdin: {}", e))
                })?;
                buf
            } else if let Some(path) = last.strip_prefix('@') {
                std::fs::read_to_string(path).map_err(|e| {
                    crate::SpallCliError::Usage(format!("Failed to read file {}: {}", path, e))
                })?
            } else {
                last.clone()
            };

            let ct = phase2_matches
                .get_one::<String>("spall-content-type")
                .cloned()
                .unwrap_or_else(|| "application/json".to_string());

            return Ok(BodyData {
                content_type: Some(ct),
                body: Some(data.into_bytes()),
                multipart: None,
            });
        }
    }

    // --form (multipart, Wave 1.5)
    if let Some(values) = phase2_matches.get_many::<String>("form") {
        let mut form = reqwest::multipart::Form::new();
        for val in values {
            if let Some((key, rest)) = val.split_once('=') {
                if let Some(path) = rest.strip_prefix('@') {
                    let content = std::fs::read(path).map_err(|e| {
                        crate::SpallCliError::Usage(format!("Failed to read file {}: {}", path, e))
                    })?;
                    let part = reqwest::multipart::Part::bytes(content).file_name(path.to_string());
                    form = form.part(key.to_string(), part);
                } else {
                    form = form.text(key.to_string(), rest.to_string());
                }
            }
        }
        return Ok(BodyData {
            content_type: Some("multipart/form-data".to_string()),
            body: None,
            multipart: Some(form),
        });
    }

    // --field (form-urlencoded, Wave 1.5)
    if let Some(values) = phase2_matches.get_many::<String>("field") {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for val in values {
            if let Some((key, value)) = val.split_once('=') {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        let encoded = urlencoding::encode(
            &pairs
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("&"),
        )
        .to_string();
        return Ok(BodyData {
            content_type: Some("application/x-www-form-urlencoded".to_string()),
            body: Some(encoded.into_bytes()),
            multipart: None,
        });
    }

    Ok(BodyData {
        content_type: None,
        body: None,
        multipart: None,
    })
}

#[allow(clippy::too_many_arguments)]
/// Record a request to the history database, redacting sensitive headers.
fn record_history(
    cache_dir: &std::path::Path,
    api: &str,
    operation: &str,
    method: &str,
    url: &str,
    status: Option<reqwest::StatusCode>,
    req_headers: &HeaderMap,
    resp_headers: &HeaderMap,
    duration_ms: u64,
) {
    let history = match crate::history::History::open(cache_dir) {
        Ok(h) => h,
        Err(_) => return,
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let request_headers: Vec<(String, String)> = req_headers
        .iter()
        .map(|(k, v)| {
            let val = if crate::history::is_sensitive_header(k.as_str()) {
                "[REDACTED]".to_string()
            } else {
                v.to_str().unwrap_or("?").to_string()
            };
            (k.to_string(), val)
        })
        .collect();

    let response_headers: Vec<(String, String)> = resp_headers
        .iter()
        .map(|(k, v)| {
            let val = if crate::history::is_sensitive_header(k.as_str()) {
                "[REDACTED]".to_string()
            } else {
                v.to_str().unwrap_or("?").to_string()
            };
            (k.to_string(), val)
        })
        .collect();

    let record = crate::history::RequestRecord {
        timestamp,
        api: api.to_string(),
        operation: operation.to_string(),
        method: method.to_string(),
        url: url.to_string(),
        status_code: status.map(|s| s.as_u16()).unwrap_or(0),
        duration_ms,
        request_headers,
        response_headers,
    };

    let _ = history.record(&record);
}

#[cfg(test)]
mod programmatic_tests {
    //! Inline integration tests for [`execute_operation_programmatic`].
    //!
    //! These guard against drift between the clap-driven path and the
    //! programmatic path: both must produce the same outbound request
    //! shape given the same inputs. Each test builds a `ResolvedSpec`
    //! and `ApiEntry` in-process, points them at a `wiremock` server,
    //! and asserts the on-wire request matches expectations.
    use super::*;
    use indexmap::IndexMap;
    use spall_config::registry::ApiEntry;
    use spall_core::ir::{
        HttpMethod, ParameterLocation, ResolvedOperation, ResolvedParameter, ResolvedSchema,
        ResolvedSpec,
    };
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn schema_any() -> ResolvedSchema {
        ResolvedSchema {
            type_name: None,
            format: None,
            description: None,
            default: None,
            enum_values: Vec::new(),
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

    fn path_param(name: &str) -> ResolvedParameter {
        ResolvedParameter {
            name: name.to_string(),
            location: ParameterLocation::Path,
            required: true,
            deprecated: false,
            style: String::new(),
            explode: false,
            schema: schema_any(),
            description: None,
            extensions: IndexMap::new(),
        }
    }

    fn make_get_op(op_id: &str, path_template: &str, with_path_param: Option<&str>) -> ResolvedOperation {
        let parameters = match with_path_param {
            Some(name) => vec![path_param(name)],
            None => Vec::new(),
        };
        ResolvedOperation {
            operation_id: op_id.to_string(),
            method: HttpMethod::Get,
            path_template: path_template.to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters,
            request_body: None,
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        }
    }

    fn make_post_op(op_id: &str, path_template: &str) -> ResolvedOperation {
        let mut op = make_get_op(op_id, path_template, None);
        op.method = HttpMethod::Post;
        op
    }

    fn make_spec(base_url: &str) -> ResolvedSpec {
        ResolvedSpec {
            title: "test".to_string(),
            version: "1.0.0".to_string(),
            base_url: base_url.to_string(),
            operations: Vec::new(),
            servers: vec![spall_core::ir::ResolvedServer {
                url: base_url.to_string(),
                description: None,
            }],
        }
    }

    fn make_entry(name: &str, base_url: &str) -> ApiEntry {
        ApiEntry {
            name: name.to_string(),
            source: format!("{}/openapi.json", base_url),
            config_path: None,
            base_url: Some(base_url.to_string()),
            default_headers: Vec::new(),
            auth: None,
            proxy: None,
            profiles: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn programmatic_get_with_path_query_header_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/items/abc-123"))
            .and(wiremock::matchers::query_param("filter", "active"))
            .and(header("x-trace-id", "trace-xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "abc-123",
                "count": 7,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = make_spec(&base);
        let entry = make_entry("test", &base);
        let op = make_get_op("getItem", "/items/{id}", Some("id"));

        let mut args = ProgrammaticArgs::new();
        args.path.insert("id".to_string(), "abc-123".to_string());
        args.query
            .insert("filter".to_string(), "active".to_string());
        args.header
            .insert("X-Trace-Id".to_string(), "trace-xyz".to_string());

        let result = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("request");
        assert_eq!(result.status.as_u16(), 200);
        assert_eq!(result.value, serde_json::json!({"id": "abc-123", "count": 7}));
    }

    #[tokio::test]
    async fn programmatic_post_serializes_json_body_and_sets_content_type() {
        let server = MockServer::start().await;
        let body = serde_json::json!({"email": "a@b.test", "n": 3});
        Mock::given(method("POST"))
            .and(path("/login"))
            .and(header("content-type", "application/json"))
            .and(body_json(&body))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = make_spec(&base);
        let entry = make_entry("test", &base);
        let op = make_post_op("login", "/login");

        let mut args = ProgrammaticArgs::new();
        args.body = Some(body.clone());

        let res = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("post");
        assert_eq!(res.status.as_u16(), 201);
        assert_eq!(res.value, serde_json::json!({"ok": true}));
    }

    #[tokio::test]
    async fn programmatic_caller_header_wins_over_auth_chain() {
        // Auth is unconfigured on the ApiEntry, so the auth chain returns
        // None and the caller's Authorization header is the only one set.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer caller-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"u": 1})))
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = make_spec(&base);
        let entry = make_entry("test", &base);
        let op = make_get_op("getMe", "/me", None);

        let mut args = ProgrammaticArgs::new();
        args.header
            .insert("Authorization".to_string(), "Bearer caller-token".to_string());

        let res = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("get");
        assert_eq!(res.status.as_u16(), 200);
    }

    #[tokio::test]
    async fn programmatic_4xx_returns_ok_with_status_and_raise_lifts_to_err() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/forbidden"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-error-id", "trace-9")
                    .set_body_json(serde_json::json!({"detail": "nope"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = make_spec(&base);
        let entry = make_entry("test", &base);
        let op = make_get_op("forbidden", "/forbidden", None);
        let args = ProgrammaticArgs::new();

        let res = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("transport ok");
        assert_eq!(res.status.as_u16(), 403);
        // Headers + body are observable on failure so callers can record
        // history, build MCP error payloads, or hint at remediation.
        assert_eq!(res.headers.get("x-error-id").map(String::as_str), Some("trace-9"));
        assert_eq!(res.value, serde_json::json!({"detail": "nope"}));
        // raise_for_status lifts the result into Err for callers that
        // want the historical 4xx-as-error contract.
        let err = raise_for_status(res).expect_err("expected 4xx error");
        assert!(matches!(err, crate::SpallCliError::Http4xx(403)));
    }

    #[tokio::test]
    async fn programmatic_default_has_one_retry() {
        // Regression: ProgrammaticArgs::default() used to give 0 retries
        // (only ::new() applied sensible defaults). The runner's
        // BTreeMap-of-args + ..Default::default() patterns relied on the
        // defaults being correct, so this is a real call-site footgun.
        let args = ProgrammaticArgs::default();
        assert_eq!(args.retry_count, 1);
        assert_eq!(args.retry_max_wait_secs, 60);
    }

    #[tokio::test]
    async fn programmatic_server_override_supersedes_entry_base() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let real_base = server.uri();
        let spec = make_spec("http://wrong.invalid");
        let entry = make_entry("test", "http://also-wrong.invalid");
        let op = make_get_op("listItems", "/v2/items", None);

        let mut args = ProgrammaticArgs::new();
        args.server_override = Some(real_base);
        let res = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("get");
        assert_eq!(res.status.as_u16(), 200);
    }

    // ============================================================
    // Drift guard: clap-driven `execute_operation` and the
    // programmatic `execute_operation_programmatic` MUST emit the same
    // outbound request shape for equivalent logical inputs. They share
    // `prepare_and_send`; this test pins that sharing in place so a
    // future refactor that re-introduces a parallel pipeline trips
    // immediately. If you have to change a matcher below, you almost
    // certainly need to update both paths together.
    // ============================================================

    fn make_post_op_three_params() -> ResolvedOperation {
        let mut content = IndexMap::new();
        content.insert(
            "application/json".to_string(),
            spall_core::ir::ResolvedMediaType {
                schema: Some(schema_any()),
                example: None,
                examples: IndexMap::new(),
            },
        );
        ResolvedOperation {
            operation_id: "createItem".to_string(),
            method: HttpMethod::Post,
            path_template: "/items/{id}".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![
                ResolvedParameter {
                    name: "id".to_string(),
                    location: ParameterLocation::Path,
                    required: true,
                    deprecated: false,
                    style: String::new(),
                    explode: false,
                    schema: schema_any(),
                    description: None,
                    extensions: IndexMap::new(),
                },
                ResolvedParameter {
                    name: "filter".to_string(),
                    location: ParameterLocation::Query,
                    required: true,
                    deprecated: false,
                    style: String::new(),
                    explode: false,
                    schema: schema_any(),
                    description: None,
                    extensions: IndexMap::new(),
                },
                ResolvedParameter {
                    name: "X-Trace-Id".to_string(),
                    location: ParameterLocation::Header,
                    required: true,
                    deprecated: false,
                    style: String::new(),
                    explode: false,
                    schema: schema_any(),
                    description: None,
                    extensions: IndexMap::new(),
                },
            ],
            // The clap path's resolve_body short-circuits when this is
            // None, even if --data was supplied. Declaring a body here
            // matches what a real OpenAPI spec for createItem would
            // describe and lets --data flow through.
            request_body: Some(spall_core::ir::ResolvedRequestBody {
                description: None,
                required: true,
                content,
            }),
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        }
    }

    /// Mirror of the arg shape `spall_core::command::build_operations_cmd`
    /// would emit: `path-{name}`, `query-{name}`, `header-{name}`, plus
    /// the body knobs `--data` / `--no-data` / `--spall-content-type`.
    /// We also register the **typed** global args (spall-timeout u64,
    /// spall-retry u8, etc.) the executor reads via `MergedMatches` —
    /// without these registrations, clap would panic on type-tagged
    /// `get_one` calls. Self-contained so a future change to the
    /// dynamic builder doesn't silently re-shape what we hand to
    /// `execute_operation`.
    fn build_phase2_for_create_item() -> clap::ArgMatches {
        with_global_typed_args(clap::Command::new("createItem"))
            .arg(clap::Arg::new("path-id").long("id").required(true))
            .arg(clap::Arg::new("query-filter").long("filter").required(true))
            .arg(
                clap::Arg::new("header-X-Trace-Id")
                    .long("trace")
                    .required(true),
            )
            .arg(
                clap::Arg::new("data")
                    .long("data")
                    .action(clap::ArgAction::Append)
                    .num_args(1),
            )
            .arg(
                clap::Arg::new("spall-content-type")
                    .long("spall-content-type")
                    .num_args(1),
            )
            .try_get_matches_from([
                "createItem",
                "--id",
                "abc-123",
                "--filter",
                "active",
                "--trace",
                "trace-xyz",
                "--data",
                r#"{"label":"hello","n":42}"#,
            ])
            .expect("phase2 matches build")
    }

    fn build_phase1_empty() -> clap::ArgMatches {
        with_global_typed_args(clap::Command::new("spall"))
            .try_get_matches_from(["spall"])
            .expect("phase1 matches build")
    }

    /// Register the typed global args the executor reads. Mirrors the
    /// subset of `spall_global_args()` in `main.rs` that the parity
    /// path actually touches — keeping it minimal so a new global flag
    /// being added to `main.rs` doesn't auto-fail this test.
    fn with_global_typed_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new("spall-timeout")
                .long("spall-timeout")
                .num_args(1)
                .value_parser(clap::value_parser!(u64)),
        )
        .arg(
            clap::Arg::new("spall-retry")
                .long("spall-retry")
                .num_args(1)
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            clap::Arg::new("spall-retry-max-wait")
                .long("spall-retry-max-wait")
                .num_args(1)
                .value_parser(clap::value_parser!(u64)),
        )
        .arg(
            clap::Arg::new("spall-redirect")
                .long("spall-redirect")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-insecure")
                .long("spall-insecure")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-max-redirects")
                .long("spall-max-redirects")
                .num_args(1)
                .value_parser(clap::value_parser!(usize)),
        )
        .arg(
            clap::Arg::new("spall-server")
                .long("spall-server")
                .num_args(1),
        )
        .arg(clap::Arg::new("spall-auth").long("spall-auth").num_args(1))
        .arg(
            clap::Arg::new("spall-header")
                .long("spall-header")
                .action(clap::ArgAction::Append)
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-dry-run")
                .long("spall-dry-run")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-preview")
                .long("spall-preview")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-paginate")
                .long("spall-paginate")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-verbose")
                .long("spall-verbose")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-time")
                .long("spall-time")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("spall-follow")
                .long("spall-follow")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-filter")
                .long("spall-filter")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-output")
                .long("spall-output")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-download")
                .long("spall-download")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-ca-cert")
                .long("spall-ca-cert")
                .num_args(1),
        )
        .arg(clap::Arg::new("spall-cert").long("spall-cert").num_args(1))
        .arg(clap::Arg::new("spall-key").long("spall-key").num_args(1))
        .arg(
            clap::Arg::new("spall-proxy")
                .long("spall-proxy")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("spall-no-proxy")
                .long("spall-no-proxy")
                .action(clap::ArgAction::SetTrue),
        )
    }

    #[tokio::test]
    async fn parity_clap_and_programmatic_emit_identical_request() {
        let server = MockServer::start().await;
        let body = serde_json::json!({"label": "hello", "n": 42});

        // Strict matchers FIRST: same method, path, query, trace
        // header, content-type, body. We expect *exactly two* hits —
        // one per execution path. wiremock matches in registration
        // order so the strict mock gets first refusal.
        Mock::given(method("POST"))
            .and(path("/items/abc-123"))
            .and(wiremock::matchers::query_param("filter", "active"))
            .and(header("x-trace-id", "trace-xyz"))
            .and(header("content-type", "application/json"))
            .and(body_json(&body))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(2)
            .mount(&server)
            .await;

        // Permissive fallback: anything that didn't satisfy the strict
        // matcher above still gets a 200, so the diagnostic dump below
        // can show what drifted instead of dying with an opaque 404.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = ResolvedSpec {
            title: "test".to_string(),
            version: "1.0.0".to_string(),
            base_url: base.clone(),
            operations: vec![make_post_op_three_params()],
            servers: vec![spall_core::ir::ResolvedServer {
                url: base.clone(),
                description: None,
            }],
        };
        let entry = make_entry("test", &base);
        let op = make_post_op_three_params();

        // Path A: programmatic
        let mut args = ProgrammaticArgs::default();
        args.path.insert("id".to_string(), "abc-123".to_string());
        args.query.insert("filter".to_string(), "active".to_string());
        args.header
            .insert("X-Trace-Id".to_string(), "trace-xyz".to_string());
        args.body = Some(body.clone());
        let prog_res = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect("programmatic");
        assert_eq!(prog_res.status.as_u16(), 200);

        // Path B: clap-driven
        let phase1 = build_phase1_empty();
        let phase2 = build_phase2_for_create_item();
        let cache = tempfile::tempdir().expect("tempdir");
        let defaults = spall_config::sources::GlobalDefaults::default();
        let clap_res =
            execute_operation(&op, &spec, &entry, &phase2, &phase1, cache.path(), &defaults)
                .await
                .expect("clap");
        assert_eq!(clap_res.status.as_u16(), 200);

        // Cross-check by inspecting received requests directly: every
        // POST should have hit /items/abc-123, with the same headers
        // and body. Using `received_requests` (rather than only
        // .expect(2)) gives a clear diff on failure instead of an
        // opaque "expected 2, got N" at drop.
        let received = server.received_requests().await.unwrap_or_default();
        let bodies: Vec<String> = received
            .iter()
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .collect();
        assert_eq!(
            received.len(),
            2,
            "expected 2 outbound requests, got {}: {:?}",
            received.len(),
            bodies,
        );
        for (i, r) in received.iter().enumerate() {
            assert_eq!(r.method.as_str(), "POST", "request #{i} method");
            assert_eq!(r.url.path(), "/items/abc-123", "request #{i} path");
            assert_eq!(
                r.url.query(),
                Some("filter=active"),
                "request #{i} query"
            );
            assert_eq!(
                r.headers.get("x-trace-id").and_then(|v| v.to_str().ok()),
                Some("trace-xyz"),
                "request #{i} x-trace-id",
            );
            assert_eq!(
                r.headers.get("content-type").and_then(|v| v.to_str().ok()),
                Some("application/json"),
                "request #{i} content-type",
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&r.body).ok(),
                Some(body.clone()),
                "request #{i} body",
            );
        }
    }
}
