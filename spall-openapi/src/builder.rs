//! Transport-neutral request builder.
//!
//! [`build_request`] turns a [`ResolvedOperation`] plus a *neutral* argument
//! map (`param name -> SpallValue`), a base URL, and an already-resolved
//! [`RequestBody`] into an [`HttpRequestSpec`]. It is a pure spec-to-request
//! transformation: it performs no I/O, opens no files, reads no stdin, and
//! depends on no HTTP client, async runtime, CLI framework, or config types.
//!
//! ## Relationship to the CLI
//!
//! This is the extraction of the request-assembly logic that previously lived
//! in `spall-cli`'s `execute.rs` (`build_url_with_path_args` + the
//! header / cookie / query / body steps of `prepare_and_send`). The CLI keeps
//! everything around the edges — clap parsing, `--data` / `--form` / `--field`
//! plus stdin / `@file` reading, `reqwest::multipart` construction, and auth
//! resolution — and adapts its inputs into the neutral types this builder
//! consumes (wired up in issue #28). Deliberately *not* replicated here:
//!
//! * **Auth.** Bearer / API-key / basic injection is issue #26; the builder
//!   only emits the headers the spec calls for.
//! * **Caller-header-wins-over-auth ordering.** That precedence is the CLI's
//!   orchestration concern (#28); the builder simply emits the spec'd headers
//!   alongside the supplied `default_headers`.
//! * **Body materialization.** The caller passes an already-resolved
//!   [`RequestBody`]; the builder never reads files or stdin.
//!
//! ## Unknown arguments
//!
//! Any entry in `args` whose name does not match a
//! [`ResolvedParameter`](spall_core::ir::ResolvedParameter) of the operation is
//! silently ignored. Routing is driven entirely by the operation's declared
//! parameters and their [`ParameterLocation`]; an argument the spec does not
//! declare has no location to route to.

use crate::request::{Headers, HttpRequestSpec, RequestBody};
use indexmap::IndexMap;
use spall_core::ir::{ParameterLocation, ResolvedOperation, ResolvedSpec};
use spall_core::value::SpallValue;

/// A build-time failure produced by [`build_request`].
///
/// These are errors that can be detected purely from the operation, the
/// argument map, and the body — before any network activity. The only
/// build-time failure today is a missing required path parameter; the variant
/// list is open so later request-assembly checks can be added without breaking
/// callers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// A path-template parameter the operation marks `required` had no value in
    /// the argument map, so the URL could not be fully substituted. The payload
    /// is the parameter name.
    #[error("missing required path parameter '{0}'")]
    MissingPathParam(String),
}

/// Build a transport-neutral [`HttpRequestSpec`] from a resolved operation and
/// a neutral argument map.
///
/// Inputs:
///
/// * `op` — the resolved operation; its `parameters` drive routing and its
///   `path_template` / `servers` drive the URL.
/// * `spec` — the resolved spec, consulted only for its server fallback.
/// * `base_url` — the caller's already-resolved base (in the CLI this is
///   `server_override.or(entry.base_url)`); `None` falls back to the
///   operation's first server, then the spec's first server, then `"/"`.
/// * `args` — neutral argument values keyed by parameter name. Each is routed
///   by the matching parameter's [`ParameterLocation`]; unknown names are
///   ignored (see the module docs).
/// * `body` — an already-resolved request body, or `None` for a bodyless
///   request. The builder never reads files or stdin.
/// * `default_headers` — `(name, value)` header pairs to seed the request
///   (in the CLI these come from the API entry's configured default headers).
///   Names are lowercased to satisfy the [`Headers`] contract.
///
/// Routing, matching the behavior extracted from the CLI:
///
/// * **Path** params substitute into the URL template (both `{name}` and
///   `{name*}` forms, via raw `String::replace` with **no** percent-encoding,
///   preserving the original behavior). A missing required path param is a
///   [`BuildError::MissingPathParam`].
/// * **Query** params become ordered `(name, value)` pairs. A
///   [`SpallValue::Array`] value *explodes* into one pair per element
///   (OpenAPI `style: form, explode: true`, the query default).
/// * **Header** params become headers (names lowercased).
/// * **Cookie** params become `(name, value)` entries in
///   [`HttpRequestSpec::cookies`]; the transport joins them into a single
///   `Cookie` header per its own policy.
///
/// Body handling mirrors the CLI's content-type step: for any body kind the
/// builder sets the matching `content-type` header **only if no `content-type`
/// header is already present** (e.g. one supplied via a header parameter), so a
/// spec-declared content type is never clobbered.
///
/// Auth and caller-header precedence are intentionally out of scope (see the
/// module docs).
///
/// # Errors
///
/// Returns [`BuildError::MissingPathParam`] when a `required` path parameter is
/// absent from `args`.
#[must_use = "the assembled HttpRequestSpec is the only output"]
pub fn build_request(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    base_url: Option<&str>,
    args: &IndexMap<String, SpallValue>,
    body: Option<RequestBody>,
    default_headers: &[(String, String)],
) -> Result<HttpRequestSpec, BuildError> {
    // Index the operation's parameters by name so each argument can be routed
    // by its declared location. The IR forbids duplicate (name, in) tuples, so
    // first-wins on name collisions across locations is acceptable and matches
    // the per-location lookups the CLI performed.
    let by_name: IndexMap<&str, &spall_core::ir::ResolvedParameter> =
        op.parameters.iter().map(|p| (p.name.as_str(), p)).collect();

    // --- URL (replicates build_url_with_path_args) -----------------------
    let url = build_url(op, spec, base_url, args, &by_name)?;

    // --- Headers ----------------------------------------------------------
    let mut headers: Headers = Headers::new();

    // Default headers from the caller (API-entry config in the CLI). Names are
    // lowercased to honor the Headers contract.
    for (name, value) in default_headers {
        headers.insert(name.to_ascii_lowercase(), value.clone());
    }

    // --- Route every arg by its parameter location ------------------------
    let mut query: Vec<(String, String)> = Vec::new();
    let mut cookies: Vec<(String, String)> = Vec::new();

    for (name, value) in args {
        // Unknown args have no declared location and are ignored.
        let Some(param) = by_name.get(name.as_str()) else {
            continue;
        };
        match param.location {
            // Path params are consumed by URL substitution above.
            ParameterLocation::Path => {}
            ParameterLocation::Query => explode_query(name, value, &mut query),
            ParameterLocation::Header => {
                if let Some(s) = scalar_to_param_string(value) {
                    headers.insert(name.to_ascii_lowercase(), s);
                }
            }
            ParameterLocation::Cookie => {
                if let Some(s) = scalar_to_param_string(value) {
                    cookies.push((name.clone(), s));
                }
            }
        }
    }

    // --- Body + content-type ---------------------------------------------
    // Set the content-type for the body kind only when one is not already
    // present (e.g. supplied via a header parameter), matching prepare_and_send
    // step 6 / the legacy form/multipart content-type step.
    if let Some(b) = &body {
        if !headers.contains_key("content-type") {
            if let Some(ct) = default_content_type(b) {
                headers.insert("content-type".to_string(), ct.to_string());
            }
        }
    }

    Ok(HttpRequestSpec {
        method: op.method,
        url,
        query,
        headers,
        cookies,
        body,
    })
}

/// Resolve the base URL and substitute path parameters into the template.
///
/// This replicates `build_url_with_path_args` exactly: server precedence is
/// `base_url` then the operation's first server then the spec's first server
/// then `"/"`; each present path parameter replaces both `{name}` and `{name*}`
/// via raw `String::replace` with no percent-encoding; the base's trailing
/// slashes are trimmed and the path is forced to start with `/` before
/// concatenation. A required path parameter absent from `args` is an error.
fn build_url(
    op: &ResolvedOperation,
    spec: &ResolvedSpec,
    base_url: Option<&str>,
    args: &IndexMap<String, SpallValue>,
    by_name: &IndexMap<&str, &spall_core::ir::ResolvedParameter>,
) -> Result<String, BuildError> {
    let base = base_url
        .map(str::to_string)
        .or_else(|| op.servers.first().map(|s| s.url.clone()))
        .or_else(|| spec.servers.first().map(|s| s.url.clone()))
        .unwrap_or_else(|| "/".to_string());

    let mut path = op.path_template.clone();
    for param in &op.parameters {
        if param.location != ParameterLocation::Path {
            continue;
        }
        match args
            .get(param.name.as_str())
            .and_then(scalar_to_param_string)
        {
            Some(v) => {
                path = path.replace(&format!("{{{}}}", param.name), &v);
                path = path.replace(&format!("{{{}*}}", param.name), &v);
            }
            None => {
                // Re-check the parameter's required flag via the index so a
                // by-name lookup mirrors the operation's declared parameter.
                let required = by_name.get(param.name.as_str()).is_some_and(|p| p.required);
                if required {
                    return Err(BuildError::MissingPathParam(param.name.clone()));
                }
            }
        }
    }

    let base_trimmed = base.trim_end_matches('/');
    let path_normalized = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    Ok(format!("{base_trimmed}{path_normalized}"))
}

/// Expand a query parameter value into one or more `(name, value)` pairs.
///
/// A [`SpallValue::Array`] explodes element-by-element into repeated pairs
/// (OpenAPI `style: form, explode: true`, the query default), preserving
/// element order; this subsumes the CLI's old `query_extras` mechanism. A
/// scalar value yields a single pair (skipped entirely when it stringifies to
/// nothing, i.e. [`SpallValue::Null`]). Nested arrays / objects inside an array
/// element are not flattened further — they stringify via their element form.
fn explode_query(name: &str, value: &SpallValue, out: &mut Vec<(String, String)>) {
    match value {
        SpallValue::Array(items) => {
            for item in items {
                if let Some(s) = scalar_to_param_string(item) {
                    out.push((name.to_string(), s));
                }
            }
        }
        scalar => {
            if let Some(s) = scalar_to_param_string(scalar) {
                out.push((name.to_string(), s));
            }
        }
    }
}

/// Render a [`SpallValue`] scalar as a parameter string.
///
/// Crucially this yields the **raw** string for [`SpallValue::Str`] — *not* the
/// JSON-quoted form that [`SpallValue`]'s `Display` produces (Display routes
/// through `serde_json`, which wraps strings in quotes). Numbers use their
/// numeric `Display`, booleans render as `"true"` / `"false"`, and
/// [`SpallValue::Null`] returns `None` so the caller can skip emitting the
/// parameter entirely.
///
/// Composite values ([`SpallValue::Array`] / [`SpallValue::Object`]) fall back
/// to their JSON `Display` form; query arrays are exploded element-by-element
/// before reaching this function (see [`explode_query`]), so a top-level array
/// only lands here for non-query locations, where comma/JSON handling is the
/// caller's concern.
fn scalar_to_param_string(v: &SpallValue) -> Option<String> {
    match v {
        SpallValue::Null => None,
        SpallValue::Str(s) => Some(s.clone()),
        SpallValue::Bool(b) => Some(b.to_string()),
        SpallValue::I64(i) => Some(i.to_string()),
        SpallValue::U64(u) => Some(u.to_string()),
        SpallValue::F64(f) => Some(f.to_string()),
        // Composite values have no single canonical scalar form; fall back to
        // the JSON Display (which DOES quote strings, but only inside the
        // composite — matching what a transport would otherwise send).
        SpallValue::Array(_) | SpallValue::Object(_) => Some(v.to_string()),
    }
}

/// The default `Content-Type` for a body kind, used only when no content-type
/// header is already present.
///
/// `Json` → `application/json` (matching `prepare_and_send` step 6). `Form` →
/// `application/x-www-form-urlencoded`. `Bytes` carries its own explicit
/// content type. `Multipart` returns `None`: a multipart content type must
/// carry the boundary the transport generates, so the builder leaves it for the
/// transport to set rather than emitting a boundary-less header here.
fn default_content_type(body: &RequestBody) -> Option<&str> {
    match body {
        RequestBody::Json(_) => Some("application/json"),
        RequestBody::Form(_) => Some("application/x-www-form-urlencoded"),
        RequestBody::Bytes { content_type, .. } => Some(content_type),
        RequestBody::Multipart(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spall_core::ir::{
        HttpMethod, ParameterLocation, ResolvedOperation, ResolvedParameter, ResolvedSchema,
        ResolvedServer, ResolvedSpec,
    };

    fn bare_schema() -> ResolvedSchema {
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

    fn param(name: &str, loc: ParameterLocation, required: bool) -> ResolvedParameter {
        ResolvedParameter {
            name: name.to_string(),
            location: loc,
            required,
            deprecated: false,
            style: "form".to_string(),
            explode: true,
            schema: bare_schema(),
            description: None,
            extensions: IndexMap::new(),
        }
    }

    fn op(path_template: &str, params: Vec<ResolvedParameter>) -> ResolvedOperation {
        ResolvedOperation {
            operation_id: "op".into(),
            method: HttpMethod::Get,
            path_template: path_template.into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: params,
            request_body: None,
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        }
    }

    fn spec(servers: Vec<&str>) -> ResolvedSpec {
        ResolvedSpec {
            title: "t".into(),
            version: "1".into(),
            base_url: String::new(),
            operations: Vec::new(),
            servers: servers
                .into_iter()
                .map(|u| ResolvedServer {
                    url: u.to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    fn args(pairs: &[(&str, SpallValue)]) -> IndexMap<String, SpallValue> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    /// Full request: path + query + header + cookie params + a JSON body.
    #[test]
    fn assembles_full_request_spec() {
        let mut o = op(
            "/users/{id}/posts",
            vec![
                param("id", ParameterLocation::Path, true),
                param("limit", ParameterLocation::Query, false),
                param("X-Trace", ParameterLocation::Header, false),
                param("session", ParameterLocation::Cookie, false),
            ],
        );
        o.request_body = None; // body passed explicitly below
        let a = args(&[
            ("id", SpallValue::U64(42)),
            ("limit", SpallValue::I64(10)),
            ("X-Trace", SpallValue::Str("abc".into())),
            ("session", SpallValue::Str("xyz".into())),
        ]);
        let body = Some(RequestBody::Json(json!({"name": "ada"})));

        let req = build_request(
            &o,
            &spec(vec![]),
            Some("https://api.example.com/v2/"),
            &a,
            body,
            &[("Accept".to_string(), "application/json".to_string())],
        )
        .expect("build should succeed");

        assert_eq!(req.method, HttpMethod::Get);
        assert_eq!(req.url, "https://api.example.com/v2/users/42/posts");
        assert_eq!(req.query, vec![("limit".to_string(), "10".to_string())]);
        // Default header (lowercased) + header param (lowercased) + JSON content-type.
        assert_eq!(
            req.headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(req.headers.get("x-trace").map(String::as_str), Some("abc"));
        assert_eq!(
            req.headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            req.cookies,
            vec![("session".to_string(), "xyz".to_string())]
        );
        match req.body {
            Some(RequestBody::Json(v)) => assert_eq!(v, json!({"name": "ada"})),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    /// An array-valued query parameter explodes into one repeated pair per
    /// element, preserving order (OpenAPI form/explode default).
    #[test]
    fn array_query_explodes_to_repeated_pairs() {
        let o = op(
            "/items",
            vec![param("ids", ParameterLocation::Query, false)],
        );
        let a = args(&[(
            "ids",
            SpallValue::Array(vec![
                SpallValue::I64(1),
                SpallValue::I64(2),
                SpallValue::I64(3),
            ]),
        )]);
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &a, None, &[])
            .expect("build should succeed");
        assert_eq!(
            req.query,
            vec![
                ("ids".to_string(), "1".to_string()),
                ("ids".to_string(), "2".to_string()),
                ("ids".to_string(), "3".to_string()),
            ]
        );
    }

    /// Server precedence: `base_url = None` falls back to the operation's first
    /// server, then the spec's first server.
    #[test]
    fn server_precedence_op_then_spec() {
        // base_url None, op has a server → op server wins.
        let mut o = op("/x", vec![]);
        o.servers = vec![ResolvedServer {
            url: "https://op-server".into(),
            description: None,
        }];
        let req = build_request(
            &o,
            &spec(vec!["https://spec-server"]),
            None,
            &args(&[]),
            None,
            &[],
        )
        .expect("build should succeed");
        assert_eq!(req.url, "https://op-server/x");

        // base_url None, op has no server → spec server wins.
        let o2 = op("/x", vec![]);
        let req2 = build_request(
            &o2,
            &spec(vec!["https://spec-server"]),
            None,
            &args(&[]),
            None,
            &[],
        )
        .expect("build should succeed");
        assert_eq!(req2.url, "https://spec-server/x");

        // base_url None, neither op nor spec has a server → "/" default.
        let o3 = op("/x", vec![]);
        let req3 = build_request(&o3, &spec(vec![]), None, &args(&[]), None, &[])
            .expect("build should succeed");
        assert_eq!(req3.url, "/x");
    }

    /// A required path parameter absent from `args` is a build error.
    #[test]
    fn missing_required_path_param_errors() {
        let o = op(
            "/users/{id}",
            vec![param("id", ParameterLocation::Path, true)],
        );
        let err = build_request(&o, &spec(vec![]), Some("https://h"), &args(&[]), None, &[])
            .expect_err("missing required path param should error");
        match err {
            BuildError::MissingPathParam(name) => assert_eq!(name, "id"),
        }
    }

    /// An optional path parameter absent from `args` leaves the template
    /// placeholder untouched rather than erroring.
    #[test]
    fn missing_optional_path_param_leaves_placeholder() {
        let o = op(
            "/users/{id}",
            vec![param("id", ParameterLocation::Path, false)],
        );
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &args(&[]), None, &[])
            .expect("optional path param should not error");
        assert_eq!(req.url, "https://h/users/{id}");
    }

    /// A `{name*}` matrix/explode template form is substituted just like the
    /// plain `{name}` form (raw replace, no encoding).
    #[test]
    fn matrix_explode_template_name_substitutes() {
        let o = op(
            "/pets/{petId*}",
            vec![param("petId", ParameterLocation::Path, true)],
        );
        let a = args(&[("petId", SpallValue::Str("fido".into()))]);
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &a, None, &[])
            .expect("build should succeed");
        assert_eq!(req.url, "https://h/pets/fido");
    }

    /// The content type is NOT overridden when the operation already sets one
    /// via a header parameter.
    #[test]
    fn content_type_not_overridden_by_header_param() {
        let o = op(
            "/x",
            vec![param("Content-Type", ParameterLocation::Header, false)],
        );
        let a = args(&[(
            "Content-Type",
            SpallValue::Str("application/vnd.custom+json".into()),
        )]);
        let req = build_request(
            &o,
            &spec(vec![]),
            Some("https://h"),
            &a,
            Some(RequestBody::Json(json!({}))),
            &[],
        )
        .expect("build should succeed");
        assert_eq!(
            req.headers.get("content-type").map(String::as_str),
            Some("application/vnd.custom+json")
        );
    }

    /// A `Str` query value does not gain surrounding JSON quotes (it must use
    /// the raw string, not `SpallValue`'s quoting `Display`).
    #[test]
    fn str_query_value_has_no_quotes() {
        let o = op("/x", vec![param("q", ParameterLocation::Query, false)]);
        let a = args(&[("q", SpallValue::Str("hello world".into()))]);
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &a, None, &[])
            .expect("build should succeed");
        assert_eq!(
            req.query,
            vec![("q".to_string(), "hello world".to_string())]
        );
        // Defensive: confirm the value is NOT the quoted JSON form.
        assert_ne!(req.query[0].1, "\"hello world\"");
    }

    /// Unknown argument names (not declared as parameters) are ignored.
    #[test]
    fn unknown_args_are_ignored() {
        let o = op("/x", vec![param("known", ParameterLocation::Query, false)]);
        let a = args(&[
            ("known", SpallValue::Str("v".into())),
            ("mystery", SpallValue::Str("ignored".into())),
        ]);
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &a, None, &[])
            .expect("build should succeed");
        assert_eq!(req.query, vec![("known".to_string(), "v".to_string())]);
    }

    /// A `Form` body sets the urlencoded content type when none is present.
    #[test]
    fn form_body_sets_urlencoded_content_type() {
        let o = op("/x", vec![]);
        let body = Some(RequestBody::Form(vec![("a".into(), "b".into())]));
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &args(&[]), body, &[])
            .expect("build should succeed");
        assert_eq!(
            req.headers.get("content-type").map(String::as_str),
            Some("application/x-www-form-urlencoded")
        );
    }

    /// A `Bytes` body carries its own explicit content type when none is set.
    #[test]
    fn bytes_body_sets_its_explicit_content_type() {
        let o = op("/x", vec![]);
        let body = Some(RequestBody::Bytes {
            content_type: "text/csv".into(),
            data: b"a,b,c".to_vec(),
        });
        let req = build_request(&o, &spec(vec![]), Some("https://h"), &args(&[]), body, &[])
            .expect("build should succeed");
        assert_eq!(
            req.headers.get("content-type").map(String::as_str),
            Some("text/csv")
        );
    }
}
