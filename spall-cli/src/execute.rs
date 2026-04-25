use clap::ArgMatches;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, COOKIE};
use secrecy::ExposeSecret;
use spall_config::credentials::CredentialResolver;
use spall_config::registry::ApiEntry;
use spall_core::ir::{HttpMethod, ParameterLocation, ResolvedOperation, ResolvedRequestBody};

use std::io::Read;

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

/// Execute a matched operation.
pub async fn execute_operation(
    op: &ResolvedOperation,
    entry: &ApiEntry,
    phase2_matches: &ArgMatches,
    phase1_matches: &ArgMatches,
) -> Result<(), crate::SpallCliError> {
    let mut url = build_url(op, entry, phase1_matches, phase2_matches)?;

    let mut headers = HeaderMap::new();

    // Default headers from config
    for (k, v) in &entry.default_headers {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap_or_else(|_| HeaderName::from_static("x-unknown")),
            HeaderValue::from_str(v).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
        );
    }

    // Custom headers from --spall-header
    let combined = merge_matches(phase1_matches, phase2_matches);
    if let Some(values) = combined.get_many::<String>("spall-header") {
        for h in values {
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(
                    HeaderName::from_bytes(k.trim().as_bytes()).unwrap_or_else(|_| HeaderName::from_static("x-unknown")),
                    HeaderValue::from_str(v.trim()).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
                );
            }
        }
    }

    // Authentication
    let auth = resolve_auth(entry, &combined);
    if let Some(token) = auth {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(token.expose_secret()).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
        );
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
        headers.insert(COOKIE, HeaderValue::from_str(&cookies.join("; ")).unwrap_or_else(|_| HeaderValue::from_static("")));
    }

    // Query params
    let mut query_pairs: Vec<(&str, &str)> = Vec::new();
    for param in &op.parameters {
        if param.location == ParameterLocation::Query {
            let id = format!("query-{}", param.name);
            if let Some(v) = phase2_matches.get_one::<String>(&id) {
                query_pairs.push((&param.name, v.as_str()));
            }
        }
    }

    // Request body
    let body_data = resolve_body(op.request_body.as_ref(), phase2_matches)?;
    if let Some(ref content_type) = body_data.content_type {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
    }

    // Build client
    let http_config = crate::http::config_from_matches(phase1_matches, phase2_matches);
    let client = crate::http::build_http_client(&http_config).map_err(|e| {
        crate::SpallCliError::HttpClient(e.to_string())
    })?;

    // Build request
    let mut req_builder = match op.method {
        HttpMethod::Get => client.get(&url),
        HttpMethod::Post => client.post(&url),
        HttpMethod::Put => client.put(&url),
        HttpMethod::Delete => client.delete(&url),
        HttpMethod::Patch => client.patch(&url),
        HttpMethod::Head => client.head(&url),
        HttpMethod::Options => client.request(reqwest::Method::OPTIONS, &url),
        HttpMethod::Trace => client.request(reqwest::Method::TRACE, &url),
    };

    req_builder = req_builder.headers(headers);

    if let Some(body) = body_data.body {
        req_builder = req_builder.body(body);
    }

    if !query_pairs.is_empty() {
        req_builder = req_builder.query(&query_pairs);
    }

    // Dry run
    if combined.get_flag("spall-dry-run") {
        eprintln!("Dry run: {} {}", op.method, url);
        return Ok(());
    }

    // Send request
    let resp = req_builder.send().await.map_err(|e| {
        crate::SpallCliError::Network(e.to_string())
    })?;

    let status = resp.status();
    let body_bytes = resp.bytes().await.map_err(|e| {
        crate::SpallCliError::Network(e.to_string())
    })?;

    // Verbose output
    if combined.get_flag("spall-verbose") {
        eprintln!("HTTP {}", status);
    }

    // Output formatting
    let mode = if combined.get_flag("spall-verbose") {
        crate::output::OutputMode::Raw
    } else if let Some(output) = combined.get_one::<String>("spall-output") {
        crate::output::OutputMode::from_str(&output).unwrap_or_default()
    } else {
        crate::output::OutputMode::default()
    };

    let save_path_owned = combined.get_one::<String>("spall-download");
    let save_path = save_path_owned.as_deref();

    crate::output::emit_response(&body_bytes, mode, save_path).map_err(|e| {
        crate::SpallCliError::HttpClient(e.to_string())
    })?;

    // Exit code based on HTTP status
    if status.is_client_error() {
        std::process::exit(crate::EXIT_HTTP_4XX);
    } else if status.is_server_error() {
        std::process::exit(crate::EXIT_HTTP_5XX);
    }

    Ok(())
}

/// Merge Phase 1 and Phase 2 ArgMatches, preferring Phase 2 for overlapping values.
fn merge_matches<'a>(phase1: &'a ArgMatches, phase2: &'a ArgMatches) -> MergedMatches<'a> {
    MergedMatches { phase1, phase2 }
}

struct MergedMatches<'a> {
    phase1: &'a ArgMatches,
    phase2: &'a ArgMatches,
}

impl MergedMatches<'_> {
    fn get_flag(&self, id: &str) -> bool {
        self.phase2.get_flag(id) || self.phase1.get_flag(id)
    }

    fn get_one<T: Clone + Send + Sync + 'static>(&self, id: &str) -> Option<T> {
        self.phase2.get_one::<T>(id).cloned().or_else(|| self.phase1.get_one::<T>(id).cloned())
    }

    fn get_many<T: Clone + Send + Sync + 'static>(&self, id: &str) -> Option<clap::parser::ValuesRef<'_, T>> {
        self.phase2.get_many::<T>(id).or_else(|| self.phase1.get_many::<T>(id))
    }
}

/// Build the full request URL.
fn build_url(
    op: &ResolvedOperation,
    entry: &ApiEntry,
    phase1_matches: &ArgMatches,
    phase2_matches: &ArgMatches,
) -> Result<String, crate::SpallCliError> {
    let base = entry.base_url.clone()
        .or_else(|| phase2_matches.get_one::<String>("spall-server").cloned())
        .or_else(|| phase1_matches.get_one::<String>("spall-server").cloned())
        .unwrap_or_else(|| {
            // Parse openapi servers from spec... for Wave 1, we just use a dummy or extract from spec.
            // TODO: use ResolvedSpec.base_url directly; currently not passed to execute_operation.
            "/".to_string()
        });

    let mut path = op.path_template.clone();
    for param in &op.parameters {
        if param.location == ParameterLocation::Path {
            let id = format!("path-{}", param.name);
            if let Some(v) = phase2_matches.get_one::<String>(&id) {
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
    Ok(format!("{}{}", base_trimmed, path_trimmed))
}

/// Resolve auth from --spall-auth, env vars, or config.
fn resolve_auth(entry: &ApiEntry, matches: &MergedMatches) -> Option<secrecy::SecretString> {
    if let Some(auth) = matches.get_one::<String>("spall-auth") {
        return Some(secrecy::SecretString::new(auth.clone().into()));
    }

    let resolver = CredentialResolver {
        api_name: entry.name.clone(),
    };
    if let Ok(token) = std::env::var(resolver.env_var_name()) {
        if !token.is_empty() {
            return Some(secrecy::SecretString::new(token.into()));
        }
    }

    None
}

struct BodyData {
    content_type: Option<String>,
    body: Option<Vec<u8>>,
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
        });
    };

    // --no-data is only registered when the body is optional.
    if !body_def.required && phase2_matches.get_flag("no-data") {
        return Ok(BodyData {
            content_type: None,
            body: None,
        });
    }

    // --data
    if let Some(values) = phase2_matches.get_many::<String>("data") {
        let mut parts: Vec<String> = values.cloned().collect();
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
            });
        }
    }

    // --form (multipart, Wave 1 stub)
    if phase2_matches.get_many::<String>("form").is_some() {
        eprintln!("Warning: --form multipart upload is not yet fully implemented in Wave 1");
    }

    // --field (form-urlencoded, Wave 1 stub)
    if phase2_matches.get_many::<String>("field").is_some() {
        eprintln!("Warning: --field form-urlencoded is not yet fully implemented in Wave 1");
    }

    Ok(BodyData {
        content_type: None,
        body: None,
    })
}
