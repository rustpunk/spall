//! The reqwest transport seam.
//!
//! This is the single place that turns a transport-neutral
//! [`spall_openapi::HttpRequestSpec`] into a real `reqwest` request and sends
//! it. Everything above this module (request assembly, auth injection,
//! pagination) speaks the neutral spec; only here does `reqwest` enter.
//!
//! ## Responsibilities owned here
//!
//! * **Method / URL / query / headers / cookies / body → reqwest.** The neutral
//!   spec carries lowercased string headers and `(name, value)` query/cookie
//!   pairs; this module assembles a `reqwest::HeaderMap`, joins cookies into one
//!   `Cookie` header, and serializes each [`RequestBody`] kind to bytes.
//! * **`HeaderName` / `HeaderValue` validation fallback.** A neutral header that
//!   fails reqwest's name/value validation is skipped (matching the old
//!   `prepare_and_send` `if let Ok` behavior). This unifies the two divergent
//!   per-path fallbacks the CLI used to carry (`prepare_and_send`'s silent skip
//!   versus `execute_legacy_path`'s `x-unknown` / `invalid` substitution): the
//!   single seam now skips silently for every path.
//! * **Multipart file I/O.** A [`MultipartValue::File`] descriptor is *read from
//!   disk here* and added to the streaming `reqwest::multipart::Form`. The
//!   neutral spec only ever held a path, never the bytes; opening the file is a
//!   transport concern and lives at this boundary.
//!
//! The response body is buffered into a `Vec<u8>` (the CLI already buffered each
//! body before the extraction), so an [`spall_openapi::ItemStream`] is fed a
//! `std::io::Cursor<Vec<u8>>` rather than a live socket — no async-to-sync
//! bridge is needed.

use crate::execute::send_one;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, COOKIE};
use spall_openapi::{
    request::Headers as NeutralHeaders, HttpRequestSpec, MultipartValue, RequestBody, Status,
};

/// Send a transport-neutral request spec via reqwest and buffer the response.
///
/// Returns the response status, its lowercased headers, and the fully-buffered
/// body bytes. The retry policy (`retry_count` attempts beyond the first,
/// `Retry-After` honored up to `retry_max_wait` seconds) is applied by the
/// shared [`send_one`].
///
/// # Errors
///
/// Returns a network error from [`send_one`], or a usage error if a multipart
/// file descriptor cannot be read from disk.
pub(crate) async fn send_spec(
    client: &reqwest::Client,
    spec: &HttpRequestSpec,
    retry_count: u8,
    retry_max_wait: u64,
) -> Result<(Status, NeutralHeaders, Vec<u8>), crate::SpallCliError> {
    let mut headers = HeaderMap::new();
    for (name, value) in &spec.headers {
        // Skip any header that fails reqwest's name/value validation. This is
        // the unified fallback for every request path (see module docs).
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(n, v);
        }
    }

    // Cookies join into a single `Cookie` header, "k=v; k=v".
    if !spec.cookies.is_empty() {
        let joined = spec
            .cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if let Ok(v) = HeaderValue::from_str(&joined) {
            headers.insert(COOKIE, v);
        }
    }

    // Body: JSON / form / bytes serialize to a `Vec<u8>`; multipart becomes a
    // reqwest streaming Form (file parts are read from disk here).
    let mut body_bytes: Option<Vec<u8>> = None;
    let mut multipart: Option<reqwest::multipart::Form> = None;
    match &spec.body {
        Some(RequestBody::Json(value)) => {
            let bytes = serde_json::to_vec(value)
                .map_err(|e| crate::SpallCliError::Usage(format!("invalid JSON body: {e}")))?;
            body_bytes = Some(bytes);
        }
        Some(RequestBody::Form(pairs)) => {
            let encoded = serde_urlencoded::to_string(pairs)
                .map_err(|e| crate::SpallCliError::Usage(format!("invalid form body: {e}")))?;
            body_bytes = Some(encoded.into_bytes());
        }
        Some(RequestBody::Bytes { data, .. }) => {
            body_bytes = Some(data.clone());
        }
        Some(RequestBody::Multipart(fields)) => {
            multipart = Some(build_multipart(fields)?);
        }
        None => {}
    }

    let (status, resp_hdrs, bytes) = send_one(
        client,
        spec.method,
        &spec.url,
        headers,
        body_bytes,
        multipart,
        &spec.query,
        retry_count,
        retry_max_wait,
    )
    .await?;

    Ok((
        Status::from(status.as_u16()),
        lowercase_headers(&resp_hdrs),
        bytes,
    ))
}

/// Build a `reqwest::multipart::Form` from neutral multipart field descriptors,
/// reading file parts from disk at this transport boundary.
fn build_multipart(
    fields: &[spall_openapi::MultipartField],
) -> Result<reqwest::multipart::Form, crate::SpallCliError> {
    let mut form = reqwest::multipart::Form::new();
    for field in fields {
        match &field.value {
            MultipartValue::Text(text) => {
                form = form.text(field.name.clone(), text.clone());
            }
            MultipartValue::Bytes {
                filename,
                content_type,
                data,
            } => {
                let part = apply_mime(
                    reqwest::multipart::Part::bytes(data.clone()).file_name(filename.clone()),
                    Some(content_type),
                );
                form = form.part(field.name.clone(), part);
            }
            MultipartValue::File {
                path,
                filename,
                content_type,
            } => {
                // File I/O is a transport concern: the neutral spec only carried
                // a path. Read it here, mirroring the old resolve_body behavior.
                let content = std::fs::read(path).map_err(|e| {
                    crate::SpallCliError::Usage(format!(
                        "Failed to read file {}: {e}",
                        path.display()
                    ))
                })?;
                let name = filename
                    .clone()
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let part = apply_mime(
                    reqwest::multipart::Part::bytes(content).file_name(name),
                    content_type.as_deref(),
                );
                form = form.part(field.name.clone(), part);
            }
        }
    }
    Ok(form)
}

/// Apply an explicit MIME type to a multipart part, leaving the part unchanged
/// if the type string is absent or invalid. `Part` is not `Clone`, so the MIME
/// string is validated on a throwaway part first; only a valid type is applied
/// to the real one, never dropping the part's data. Mirrors the lenient
/// behavior of the old per-path multipart construction.
fn apply_mime(
    part: reqwest::multipart::Part,
    content_type: Option<&str>,
) -> reqwest::multipart::Part {
    match content_type {
        // Validate against a throwaway part so an invalid MIME string cannot
        // consume (and thus lose) the real part's body.
        Some(ct) if reqwest::multipart::Part::text("").mime_str(ct).is_ok() => part
            .mime_str(ct)
            .unwrap_or_else(|_| reqwest::multipart::Part::text("")),
        _ => part,
    }
}

/// Lowercase a reqwest [`HeaderMap`] into the neutral `Headers` map (RFC 9110
/// case-insensitive names). Values that are not valid UTF-8 are dropped, exactly
/// as the CLI's previous `lowercase_headers` did.
fn lowercase_headers(h: &HeaderMap) -> NeutralHeaders {
    let mut out = NeutralHeaders::new();
    for (name, value) in h.iter() {
        if let Ok(s) = value.to_str() {
            out.insert(name.as_str().to_ascii_lowercase(), s.to_string());
        }
    }
    out
}
