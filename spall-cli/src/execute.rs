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

/// Structured return value from `execute_operation`.
#[allow(dead_code)]
#[derive(Debug)]
pub struct OperationResult {
    pub status: reqwest::StatusCode,
    pub value: serde_json::Value,
}

/// Structured arguments for `execute_operation_programmatic`.
///
/// All fields are owned `String` / `Value`s; no `ArgMatches` lifetimes
/// leak through. `#[non_exhaustive]` keeps future field additions
/// non-breaking for downstream callers (the Arazzo runner, future MCP
/// dispatcher, REPL).
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct ProgrammaticArgs {
    /// Path parameters keyed by the parameter `name` from the spec.
    pub path: BTreeMap<String, String>,
    /// Query parameters keyed by name.
    pub query: BTreeMap<String, String>,
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

impl ProgrammaticArgs {
    /// Construct an empty `ProgrammaticArgs` with sensible retry defaults.
    #[must_use = "the constructed args are the only output"]
    pub fn new() -> Self {
        Self {
            retry_count: 1,
            retry_max_wait_secs: 60,
            ..Self::default()
        }
    }
}

/// Programmatic entry point into spall's request pipeline.
///
/// This is the *canonical* execution path for callers that do not have
/// `clap::ArgMatches` available — the Arazzo workflow runner, future MCP
/// server, embedded REPL drivers. The clap-driven [`execute_operation`]
/// shares the same lower-level helpers (`build_url_with_path_args`,
/// `auth::resolve`/`apply`, `send_one`), so the two paths produce
/// identical outbound requests for the same inputs.
///
/// What this **does**: URL build, header / query / cookie / path / auth
/// resolution, JSON body serialization, a single retrying `send_one`,
/// body JSON parse, 4xx/5xx → `Err`.
///
/// What this **does NOT do** (these belong in the clap wrapper):
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

    // Step 3: query pairs.
    let mut query_pairs: Vec<(String, String)> = args
        .query
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

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
    let (status, _resp_hdrs, body_bytes_resp) = send_one(
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
    .await?;

    // Step 8: surface 4xx/5xx as errors so the caller (Arazzo runner /
    // MCP dispatcher) can map them onto their own failure types.
    if status.is_client_error() {
        return Err(crate::SpallCliError::Http4xx(status.as_u16()));
    }
    if status.is_server_error() {
        return Err(crate::SpallCliError::Http5xx(status.as_u16()));
    }

    let value = serde_json::from_slice::<serde_json::Value>(&body_bytes_resp).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&body_bytes_resp).to_string())
    });
    Ok(OperationResult { status, value })
}

/// URL builder shared by the clap-driven and programmatic paths.
///
/// `server_override`, when `Some`, supersedes `entry.base_url` and the
/// per-op / per-spec server lists. `path_args` is consulted for
/// path-template substitution.
#[must_use = "the assembled URL is the only output"]
fn build_url_with_path_args(
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

/// Execute a matched operation.
pub async fn execute_operation(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    phase2_matches: &ArgMatches,
    phase1_matches: &ArgMatches,
    cache_dir: &std::path::Path,
    defaults: &spall_config::sources::GlobalDefaults,
) -> Result<OperationResult, crate::SpallCliError> {
    let url = build_url(op, spec, entry, phase1_matches, phase2_matches)?;

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
    let combined = merge_matches(phase1_matches, phase2_matches);
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

    // Authentication (Wave 3 provider dispatch)
    let cli_auth = combined.get_one::<String>("spall-auth");
    let auth = crate::auth::resolve(&entry.name, entry.auth.as_ref(), cli_auth.as_deref()).await?;
    if let Some(a) = auth {
        crate::auth::apply(&a, &mut headers, &mut query_pairs);
    }

    // Request body
    let body_data = resolve_body(op.request_body.as_ref(), phase2_matches)?;
    if let Some(ref content_type) = body_data.content_type {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
    }

    // Preflight validation (Wave 2)
    if let Err(errors) = crate::validate::preflight_validate(op, phase2_matches) {
        eprintln!("Validation failed:");
        eprintln!("{}", crate::validate::format_errors(&errors));
        return Err(crate::SpallCliError::ValidationFailed);
    }

    let start = Instant::now();

    let mut http_config = crate::http::config_from_matches(phase1_matches, phase2_matches);
    let resolved_proxy =
        crate::http::resolve_proxy(entry, defaults, phase1_matches, phase2_matches);
    http_config.proxy = resolved_proxy;

    let client = crate::http::build_http_client(&http_config)
        .map_err(crate::SpallCliError::HttpClient)?;

    // Dry run
    if combined.get_flag("spall-dry-run") {
        eprintln!("Dry run: {} {}", op.method, url);
        store_last_response(serde_json::Value::Null);
        store_last_response(serde_json::Value::Null);
        return Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: serde_json::Value::Null,
        });
    }

    // Preview (Phase D)
    if combined.get_flag("spall-preview") {
        let body_slice = body_data.body.as_deref();
        crate::preview::print_preview(&op.method.to_string(), &url, &headers, body_slice);
        store_last_response(serde_json::Value::Null);
        return Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: serde_json::Value::Null,
        });
    }

    let retry_count = combined.get_one::<u8>("spall-retry").unwrap_or(1);
    let retry_max_wait = combined.get_one::<u64>("spall-retry-max-wait").unwrap_or(60);

    let paginate = combined.get_flag("spall-paginate");

    if paginate {
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
                } else {
                    return Err(crate::SpallCliError::Http5xx(status.as_u16()));
                }
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
                // After the first request, query params are embedded in the Link URL.
                query_pairs.clear();
            } else {
                break;
            }
        }

        let final_value = paginator.concat_results(pages);

        // Response validation (warn-only, non-fatal)
        if let Some(first) = first_status {
            let ct = "application/json"; // pagination implies JSON
            let warnings = crate::validate::response_validate(op, first.as_u16(), ct, &final_value);
            if !warnings.is_empty() {
                eprintln!("Warning: response body did not match schema:");
                eprintln!("{}", crate::validate::format_errors(&warnings));
            }
        }

        // Record history for paginated request (use first page status / final URL)
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
        // Return paginated result for caller-side filtering/output
        Ok(OperationResult {
            status: reqwest::StatusCode::OK,
            value: final_value,
        })
    } else {
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

        // Record history
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

        // Response validation (warn-only)
        if status.is_success() {
            let ct = resp_headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json");
            if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                let warnings =
                    crate::validate::response_validate(op, status.as_u16(), ct, &json_val);
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
        } else if status.is_server_error() {
            return Err(crate::SpallCliError::Http5xx(status.as_u16()));
        }

        // --spall-follow <rel>: chase one hypermedia link after a successful response.
        let mut final_value = body_json;
        if let Some(rel) = combined.get_one::<String>("spall-follow") {
            let body_ref = if final_value.is_object() || final_value.is_array() {
                Some(&final_value)
            } else {
                None
            };
            let links = crate::links::Links::from_response(&resp_headers, body_ref);
            if let Some(link) = links.rel(rel.as_str()) {
                let followed_url = resolve_next_url(&url, &link.href)?;
                if combined.get_flag("spall-verbose") {
                    eprintln!("Following rel=\"{}\" -> {}", rel, followed_url);
                }
                let (fstatus, _fhdrs, fbytes) = send_one(
                    &client,
                    HttpMethod::Get,
                    &followed_url,
                    headers.clone(),
                    None,
                    None,
                    &[],
                    retry_count,
                    retry_max_wait,
                )
                .await?;
                if fstatus.is_client_error() {
                    return Err(crate::SpallCliError::Http4xx(fstatus.as_u16()));
                }
                if fstatus.is_server_error() {
                    return Err(crate::SpallCliError::Http5xx(fstatus.as_u16()));
                }
                final_value = serde_json::from_slice::<serde_json::Value>(&fbytes)
                    .unwrap_or_else(|_| {
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
        })
    }
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
    async fn programmatic_4xx_returns_http4xx_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/forbidden"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let spec = make_spec(&base);
        let entry = make_entry("test", &base);
        let op = make_get_op("forbidden", "/forbidden", None);
        let args = ProgrammaticArgs::new();

        let err = execute_operation_programmatic(&op, &spec, &entry, &args)
            .await
            .expect_err("expected 4xx error");
        assert!(matches!(err, crate::SpallCliError::Http4xx(403)));
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
}
