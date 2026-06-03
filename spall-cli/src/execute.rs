use crate::matches::MergedMatches;
use clap::ArgMatches;
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, COOKIE};
use spall_config::registry::ApiEntry;
use spall_core::ir::{
    HttpMethod, ParameterLocation, ResolvedOperation, ResolvedRequestBody, ResolvedSpec,
};
use spall_core::value::SpallValue;
use spall_openapi::request::Headers as NeutralHeaders;
use spall_openapi::{HttpRequestSpec, RequestBody, Status};

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Instant;

/// The last successful response captured for a pipe/chain consumer.
///
/// Threaded explicitly through the dispatch path so there is no cross-command
/// leak. A throwaway context is passed on paths that have no downstream
/// consumer (top-level `run`, single-command REPL dispatch); `run_piped` owns a
/// real one and reads it between stages. Only the 2xx success path writes into
/// it — dry-run, preview, and 4xx/5xx responses leave it untouched, so a stage
/// that never produced a response surfaces as a missing value rather than
/// stale data.
#[derive(Debug, Default)]
pub struct ResponseContext {
    last: Option<serde_json::Value>,
}

impl ResponseContext {
    /// Create an empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful response for the next pipe/chain stage.
    pub fn set(&mut self, value: serde_json::Value) {
        self.last = Some(value);
    }

    /// Consume the stored response, leaving the context empty.
    pub fn take(&mut self) -> Option<serde_json::Value> {
        self.last.take()
    }
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
/// `status` is the real HTTP status (read by the caller to decide the exit
/// code). `raw` is the original response body bytes, preserved verbatim for
/// byte-exact / binary / non-JSON output; it is `None` only for the
/// paginate-merged path, which has no single original body. `value` is the
/// parsed JSON (or a `from_utf8_lossy` string fallback) used for `--filter`
/// and chaining. `headers` are stored with **lowercased** names so callers can
/// do case-insensitive lookups per RFC 9110 without re-walking. Both 2xx and
/// non-success responses populate this struct; callers that want 4xx/5xx
/// surfaced as an error invoke [`raise_for_status`].
#[derive(Debug)]
pub struct OperationResult {
    pub status: Status,
    pub raw: Option<Vec<u8>>,
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
/// follow, history recording, response-context recording, verbose/time
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
        raw: Some(body_bytes),
        value,
        headers: resp_hdrs,
    })
}

/// Shared request-assembly + send pipeline.
///
/// Both the programmatic entry point above and the clap-driven
/// `execute_operation`'s single-shot branch call this. The neutral request is
/// built by `spall_openapi::build_request` (URL/path templating, query,
/// cookies, body, content-type); auth is contributed onto it; caller headers
/// are overlaid last so they win over auth; `transport::send_spec` performs the
/// single reqwest call. Centralizing this here is what prevents drift between
/// the two callers — the parity guard
/// `programmatic_tests::parity_clap_and_programmatic_emit_identical_request` in
/// this file exercises it against a wiremock server.
pub(crate) async fn prepare_and_send(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    args: &ProgrammaticArgs,
) -> Result<(Status, NeutralHeaders, Vec<u8>), crate::SpallCliError> {
    // Build the neutral request through the shared spall-openapi builder, then
    // overlay the extras `ProgrammaticArgs` carries that the spec builder does
    // not model (query_extras, auth, caller-header overrides).
    let mut req = build_neutral_spec(op, spec, entry, args)?;

    // Overlay `query_extras` after the spec-routed query pairs. These are the
    // repeated array-explosion pairs the MCP dispatcher emits (OpenAPI
    // `style: form, explode: true`); reqwest preserves the repetition.
    req.query.extend(args.query_extras.iter().cloned());

    // Auth resolution + injection happens BEFORE the caller-header overlay so a
    // caller-supplied `Authorization` wins (the historical precedence).
    let resolved_auth = crate::auth::resolve(
        &entry.name,
        entry.auth.as_ref(),
        args.auth_override.as_deref(),
    )
    .await?;
    if let Some(a) = resolved_auth {
        crate::auth::apply(&a, &mut req);
    }

    // Caller-supplied headers (spec header params + `--spall-header` overrides +
    // an explicit `Content-Type`) win over both auth and the body's default
    // content type. Lowercased to honor the neutral Headers contract.
    for (k, v) in &args.header {
        req.headers.insert(k.to_ascii_lowercase(), v.clone());
    }

    let client =
        crate::http::build_http_client(&args.http).map_err(crate::SpallCliError::HttpClient)?;
    crate::transport::send_spec(&client, &req, args.retry_count, args.retry_max_wait_secs).await
}

/// Assemble the transport-neutral [`HttpRequestSpec`] for the single-shot path.
///
/// Path parameters drive URL substitution and the JSON body (plus its default
/// content type) come from `spall_openapi::build_request`; query pairs and
/// cookies are routed per their parameter location from the disambiguated
/// `ProgrammaticArgs` buckets (so a name declared in two locations cannot
/// collide in a single name-keyed map). Header routing is deferred to the
/// caller-header overlay in [`prepare_and_send`] because spec header params and
/// `--spall-header` overrides must both land after auth.
fn build_neutral_spec(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    args: &ProgrammaticArgs,
) -> Result<HttpRequestSpec, crate::SpallCliError> {
    let base_url = args
        .server_override
        .clone()
        .or_else(|| entry.base_url.clone());

    // The builder routes args by the operation's declared parameter location.
    // Only path params go in here (they drive URL substitution); query and
    // cookie values are overlaid below from their own buckets, preserving the
    // per-location disambiguation `build_request` cannot get from one map.
    let mut arg_map: IndexMap<String, SpallValue> = IndexMap::new();
    for (k, v) in &args.path {
        arg_map.insert(k.clone(), SpallValue::Str(v.clone()));
    }

    let body = args.body.clone().map(RequestBody::Json);

    let mut req = spall_openapi::build_request(
        op,
        spec,
        base_url.as_deref(),
        &arg_map,
        body,
        &entry.default_headers,
    )
    .map_err(|e| crate::SpallCliError::Usage(e.to_string()))?;

    // Query pairs from the dedup'd `args.query` bucket (one pair per name),
    // routed here rather than through the builder so a name shared with a path
    // or cookie param keeps its own bucket.
    for (k, v) in &args.query {
        req.query.push((k.clone(), v.clone()));
    }
    // Cookie pairs likewise come from their own bucket.
    for (k, v) in &args.cookie {
        req.cookies.push((k.clone(), v.clone()));
    }

    Ok(req)
}

/// Lowercase a reqwest [`HeaderMap`] into a neutral lowercased map (RFC 9110
/// case-insensitive names). Still used by the hypermedia-follow and
/// multipart/form single-shot paths, which build reqwest `HeaderMap`s directly.
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
/// stderr logging, response-schema warnings, response-context recording)
/// around the shared [`prepare_and_send`] pipeline. The non-paginate,
/// non-multipart, non-form single-shot path delegates directly to
/// `execute_operation_programmatic`, which is the canonical pipeline
/// the Arazzo runner and future MCP server also use.
///
/// Returns `Ok(None)` for dry-run / preview (nothing to print, no response to
/// chain). Returns `Ok(Some(..))` for every real response, including 4xx/5xx —
/// the status-to-exit-code decision is the caller's, made after the body is
/// emitted. On a 2xx success the parsed value is also recorded into `sink` for
/// a downstream pipe/chain stage.
#[allow(clippy::too_many_arguments)]
pub async fn execute_operation(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    phase2_matches: &ArgMatches,
    phase1_matches: &ArgMatches,
    cache_dir: &std::path::Path,
    defaults: &spall_config::sources::GlobalDefaults,
    sink: &mut ResponseContext,
) -> Result<Option<OperationResult>, crate::SpallCliError> {
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
    // the clap → programmatic translation happens; the parity test
    // `programmatic_tests::parity_clap_and_programmatic_emit_identical_request`
    // in this file locks down the wire-level behavior.
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
                args.header
                    .insert("Content-Type".to_string(), ct.to_string());
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

    // Dry run — clap-only. Nothing to print, no response to chain, sink untouched.
    if combined.get_flag("spall-dry-run") {
        eprintln!("Dry run: {} {}", op.method, url);
        return Ok(None);
    }

    // Preview (Phase D) — clap-only.
    if combined.get_flag("spall-preview") {
        // Build a HeaderMap snapshot for preview purposes only.
        let mut preview_headers = HeaderMap::new();
        for (k, v) in &args.header {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                preview_headers.insert(name, value);
            }
        }
        let body_slice = body_data.body.as_deref();
        crate::preview::print_preview(&op.method.to_string(), &url, &preview_headers, body_slice);
        return Ok(None);
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
            sink,
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
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            resp_hdrs.insert(name, value);
        }
    }

    // Reconstruct a request-side HeaderMap for the history record.
    let mut req_hdrs_for_history = HeaderMap::new();
    for (k, v) in &args.header {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
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
        Some(result.status.as_u16()),
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

    let status = result.status;
    let mut final_value = result.value;
    let mut raw = result.raw;
    let resp_headers_lc = result.headers;

    // --spall-follow <rel>: chase one hypermedia link.
    //
    // The followed URL is absolute (per Link headers and HAL `_links`)
    // so URL assembly and path-template substitution from the original
    // operation don't apply. We hit `send_one` directly with a fresh
    // client, reusing the same auth/header set as the primary request.
    //
    // Only follow from a 2xx primary response: an error body has no
    // meaningful hypermedia links, and the caller maps the primary
    // status to the exit code after this returns.
    let follow_rel = if status.is_success() {
        combined.get_one::<String>("spall-follow")
    } else {
        None
    };
    if let Some(rel) = follow_rel {
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
            // Build a neutral GET spec for the absolute followed URL. Auth is
            // re-resolved and contributed BEFORE the caller-header overlay so
            // the follow inherits the same Authorization / ApiKey chain as the
            // primary request, with caller headers still winning — identical to
            // the pre-extraction precedence.
            let mut follow_spec = HttpRequestSpec {
                method: HttpMethod::Get,
                url: followed_url,
                query: Vec::new(),
                headers: NeutralHeaders::new(),
                cookies: Vec::new(),
                body: None,
            };
            if let Some(a) = crate::auth::resolve(
                &entry.name,
                entry.auth.as_ref(),
                args.auth_override.as_deref(),
            )
            .await?
            {
                crate::auth::apply(&a, &mut follow_spec);
            }
            for (k, v) in &args.header {
                follow_spec
                    .headers
                    .insert(k.to_ascii_lowercase(), v.clone());
            }
            let (fstatus, fhdrs, fbytes) = crate::transport::send_spec(
                &client,
                &follow_spec,
                args.retry_count,
                args.retry_max_wait_secs,
            )
            .await?;
            // A failed follow is a hard error: the followed body is not the
            // primary output, so 4xx/5xx is surfaced rather than emitted.
            let followed_value = serde_json::from_slice::<serde_json::Value>(&fbytes)
                .unwrap_or_else(|_| {
                    serde_json::Value::String(String::from_utf8_lossy(&fbytes).to_string())
                });
            let followed = raise_for_status(OperationResult {
                status: fstatus,
                raw: Some(fbytes),
                value: followed_value,
                headers: fhdrs,
            })?;
            // The followed body replaces the primary one for output, so its
            // original bytes become the `raw` the unfiltered emitter prints.
            final_value = followed.value;
            raw = followed.raw;
        } else if combined.get_flag("spall-verbose") {
            eprintln!("No link with rel=\"{}\" found in response", rel);
        }
    }

    let duration = start.elapsed();
    if combined.get_flag("spall-time") || combined.get_flag("spall-verbose") {
        eprintln!("Duration: {:?}", duration);
    }

    // Record for the next pipe/chain stage only on 2xx success; 4xx/5xx leave
    // the sink untouched so a downstream stage sees no response. Every real
    // response (including 4xx/5xx) is returned as `Some` — the caller emits the
    // body and then maps the status to the exit code.
    if status.is_success() {
        sink.set(final_value.clone());
    }
    Ok(Some(OperationResult {
        status,
        raw,
        value: final_value,
        headers: resp_headers_lc,
    }))
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
        retry_max_wait_secs: combined
            .get_one::<u64>("spall-retry-max-wait")
            .unwrap_or(60),
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
                args.header
                    .insert(k.trim().to_string(), v.trim().to_string());
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
    sink: &mut ResponseContext,
) -> Result<Option<OperationResult>, crate::SpallCliError> {
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

    // Authentication. Auth contributors mutate a neutral request spec, so
    // resolve onto a throwaway spec and merge its headers / query pairs into
    // this path's reqwest assembly. This keeps the multipart / form single-shot
    // tail on its reqwest builder while still routing auth through the one
    // shared `spall_openapi::auth` contributor set.
    let cli_auth = combined.get_one::<String>("spall-auth");
    let auth = crate::auth::resolve(&entry.name, entry.auth.as_ref(), cli_auth.as_deref()).await?;
    if let Some(a) = auth {
        let mut auth_spec = HttpRequestSpec {
            method: op.method,
            url: url.clone(),
            query: Vec::new(),
            headers: NeutralHeaders::new(),
            cookies: Vec::new(),
            body: None,
        };
        crate::auth::apply(&a, &mut auth_spec);
        for (k, v) in auth_spec.headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(&v),
            ) {
                headers.insert(name, value);
            }
        }
        query_pairs.extend(auth_spec.query);
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

    let client =
        crate::http::build_http_client(&http_config).map_err(crate::SpallCliError::HttpClient)?;

    let retry_count = combined.get_one::<u8>("spall-retry").unwrap_or(1);
    let retry_max_wait = combined
        .get_one::<u64>("spall-retry-max-wait")
        .unwrap_or(60);

    if combined.get_flag("spall-paginate") {
        if body_data.multipart.is_some() {
            return Err(crate::SpallCliError::Usage(
                "Cannot use --spall-paginate with multipart uploads".to_string(),
            ));
        }

        // Neutral headers for the paginate pages, derived once from the reqwest
        // assembly above (default headers + --spall-header + cookies + auth).
        let neutral_headers = lowercase_headers(&headers);

        let paginator = spall_openapi::Paginator::default();
        let verbose = combined.get_flag("spall-verbose");

        // Eagerly follow `rel=next` across pages, buffering each page body. This
        // preserves the old loop's semantics exactly: the first non-2xx page is
        // returned verbatim (prior pages are NOT concatenated with an error
        // body), and following stops at the page budget or an absent `rel=next`.
        // The buffered pages are then handed to the library's de-paginating
        // `ItemStream`, which flattens them via the resolved `DataPath` — the
        // streaming replacement for the old `concat_results`. All HTTP I/O stays
        // here in the async function, so no async-to-sync bridge is needed.
        let mut queued: Vec<spall_openapi::ResponseStream> = Vec::new();
        let mut current_url = url.clone();
        let mut page_query = query_pairs.clone();
        let mut first_status: Option<Status> = None;
        let mut last_url = current_url.clone();

        for page_num in 0..paginator.max_pages {
            // Only the first page carries the request body. The content type (if
            // any) is already a header in `neutral_headers`, so `Bytes` only
            // needs to carry the raw payload; the builder does not re-set the
            // content type because the header already wins.
            let page_body = if page_num == 0 {
                body_data.body.clone().map(|data| RequestBody::Bytes {
                    content_type: body_data
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    data,
                })
            } else {
                None
            };
            let page_spec = HttpRequestSpec {
                method: op.method,
                url: current_url.clone(),
                query: page_query.clone(),
                headers: neutral_headers.clone(),
                cookies: Vec::new(),
                body: page_body,
            };

            let (status, page_headers, body_bytes) =
                crate::transport::send_spec(&client, &page_spec, retry_count, retry_max_wait)
                    .await?;
            if first_status.is_none() {
                first_status = Some(status);
            }
            last_url = current_url.clone();
            if verbose {
                eprintln!("HTTP {} {}", status, current_url);
            }
            if !status.is_success() {
                // Return the error page's body verbatim for emission; the caller
                // maps the status to the exit code. Pages collected before the
                // error are intentionally not concatenated with an error body —
                // the error payload is the actionable diagnostic.
                let error_value = serde_json::from_slice::<serde_json::Value>(&body_bytes)
                    .unwrap_or_else(|_| {
                        serde_json::Value::String(String::from_utf8_lossy(&body_bytes).to_string())
                    });
                return Ok(Some(OperationResult {
                    status,
                    raw: Some(body_bytes),
                    value: error_value,
                    headers: page_headers,
                }));
            }

            let next = paginator.next_url(&page_headers);
            queued.push(spall_openapi::ResponseStream {
                status,
                headers: page_headers,
                body: Box::new(std::io::Cursor::new(body_bytes)),
            });
            match next {
                Some(n) => {
                    current_url = resolve_next_url(&current_url, &n)?;
                    // Subsequent pages carry their query in the next URL.
                    page_query.clear();
                }
                None => break,
            }
        }

        // Resolve the data path: explicit override (none from the CLI yet) >
        // `x-spall-data-path` on the operation > `ApiEntry.data_path` config >
        // `TopLevel`. The library's lenient TopLevel handling reproduces the old
        // `concat_results` shape (arrays flatten; a non-array page is one item).
        let data_path = resolve_data_path(op, entry)?;

        // Drain the de-paginating ItemStream into a single array, the streaming
        // replacement for `concat_results`. #44 will guard this collect's size.
        let final_value = collect_paginated(queued, data_path)?;

        if let Some(first) = first_status {
            let warnings = crate::validate::response_validate(
                op,
                first.as_u16(),
                "application/json",
                &final_value,
            );
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
            &last_url,
            first_status.map(Status::as_u16),
            &headers,
            &HeaderMap::new(),
            duration_ms,
        );

        // Paginate succeeded (2xx): record for the next pipe/chain stage. No
        // single original body survives the merge, so `raw` is None and the
        // caller emits the parsed value.
        sink.set(final_value.clone());
        return Ok(Some(OperationResult {
            status: Status::from(200),
            raw: None,
            value: final_value,
            headers: BTreeMap::new(),
        }));
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
        Some(status.as_u16()),
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

    // Parse for filter/chain; the utf8-lossy String is only a fallback so those
    // paths have *something*. The original bytes (`raw`) are what the unfiltered
    // output path emits, preserving binary/non-JSON content.
    let body_json = serde_json::from_slice::<serde_json::Value>(&body_bytes).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&body_bytes).to_string())
    });

    let duration = start.elapsed();
    if combined.get_flag("spall-time") || combined.get_flag("spall-verbose") {
        eprintln!("Duration: {:?}", duration);
    }

    // Record for the next pipe/chain stage only on 2xx success; 4xx/5xx leave
    // the sink untouched so a downstream stage sees no response. Every real
    // response (including 4xx/5xx) is returned as `Some` — the caller emits the
    // body and then maps the status to the exit code.
    if status.is_success() {
        sink.set(body_json.clone());
    }
    Ok(Some(OperationResult {
        status: Status::from(status.as_u16()),
        raw: Some(body_bytes),
        value: body_json,
        headers: lowercase_headers(&resp_headers),
    }))
}

/// Resolve the [`spall_openapi::DataPath`] for a paginated operation, applying
/// the precedence `x-spall-data-path` operation extension > `ApiEntry.data_path`
/// config > `DataPath::TopLevel`. (An explicit caller override is a future tier
/// and is not surfaced by the CLI yet.)
fn resolve_data_path(
    op: &ResolvedOperation,
    entry: &ApiEntry,
) -> Result<spall_openapi::DataPath, crate::SpallCliError> {
    if let Some(dp) = spall_openapi::DataPath::from_operation(op) {
        return Ok(dp);
    }
    if let Some(pointer) = &entry.data_path {
        return spall_openapi::DataPath::from_pointer(pointer).map_err(|e| {
            crate::SpallCliError::Usage(format!(
                "invalid data_path '{pointer}' for API '{}': {e}",
                entry.name
            ))
        });
    }
    Ok(spall_openapi::DataPath::TopLevel)
}

/// Drain an eagerly-fetched chain of pages through the library's de-paginating
/// [`spall_openapi::ItemStream`] into a single JSON array — the streaming
/// replacement for the old `concat_results`. The first queued page seeds the
/// stream; the rest are
/// returned in order by a fetch closure that pops the pre-fetched queue (all
/// HTTP I/O already happened in the caller, so this closure does no I/O and
/// needs no async-to-sync bridge).
fn collect_paginated(
    queued: Vec<spall_openapi::ResponseStream>,
    data_path: spall_openapi::DataPath,
) -> Result<serde_json::Value, crate::SpallCliError> {
    let mut pages = queued.into_iter();
    let Some(first) = pages.next() else {
        // No pages fetched (max_pages == 0 is impossible by default); empty array.
        return Ok(serde_json::Value::Array(Vec::new()));
    };

    let mut rest = pages;
    let fetch: spall_openapi::PageFetch = Box::new(move |_next_url: &str| {
        // The queue is already in `rel=next` order, so ignore the URL and hand
        // back the next pre-fetched page.
        rest.next().ok_or_else(|| {
            spall_openapi::StreamError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "no further pre-fetched page",
            ))
        })
    });

    let stream = spall_openapi::ItemStream::paginated(
        first,
        data_path,
        spall_openapi::Paginator::default(),
        fetch,
    );

    let mut items = Vec::new();
    for item in stream {
        let value = item.map_err(|e| {
            crate::SpallCliError::Usage(format!("Pagination requires JSON responses: {e}"))
        })?;
        items.push(value);
    }
    Ok(serde_json::Value::Array(items))
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
///
/// Byte-preserving on the unfiltered path: when no `--filter` is set and the
/// original body bytes are available (`raw`), they are emitted via
/// [`crate::output::emit_response`], which writes binary / download / raw
/// bodies verbatim and only JSON-formats parseable bodies. A `--filter`, or the
/// paginate-merged path (no single original body), falls through to
/// [`crate::output::emit_json_value`] on the parsed value.
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
                emit_unfiltered(res, mode, save_path)
            }
        }
    } else {
        emit_unfiltered(res, mode, save_path)
    }
}

/// Emit a result without a filter, preserving original bytes where possible.
fn emit_unfiltered(
    res: &OperationResult,
    mode: crate::output::OutputMode,
    save_path: Option<&str>,
) -> Result<(), crate::SpallCliError> {
    match &res.raw {
        Some(bytes) => crate::output::emit_response(bytes, mode, save_path)
            .map_err(|e| crate::SpallCliError::HttpClient(e.to_string())),
        None => crate::output::emit_json_value(&res.value, mode, save_path)
            .map_err(|e| crate::SpallCliError::HttpClient(e.to_string())),
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
    status: Option<u16>,
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
        status_code: status.unwrap_or(0),
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

    fn make_get_op(
        op_id: &str,
        path_template: &str,
        with_path_param: Option<&str>,
    ) -> ResolvedOperation {
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
            data_path: None,
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
        assert_eq!(
            result.value,
            serde_json::json!({"id": "abc-123", "count": 7})
        );
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
        args.header.insert(
            "Authorization".to_string(),
            "Bearer caller-token".to_string(),
        );

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
        assert_eq!(
            res.headers.get("x-error-id").map(String::as_str),
            Some("trace-9")
        );
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

    /// Mirror of the arg shape `crate::command::build_operations_cmd`
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
        args.query
            .insert("filter".to_string(), "active".to_string());
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
        let mut sink = ResponseContext::new();
        let clap_res = execute_operation(
            &op,
            &spec,
            &entry,
            &phase2,
            &phase1,
            cache.path(),
            &defaults,
            &mut sink,
        )
        .await
        .expect("clap")
        .expect("clap produced a response");
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
            assert_eq!(r.url.query(), Some("filter=active"), "request #{i} query");
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
