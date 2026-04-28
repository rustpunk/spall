//! Replay a request from the SQLite history database.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Instant;

/// Replay a history entry by ID, or the most recent if `id` is `None`.
pub async fn replay(
    cache_dir: &std::path::Path,
    entry_id: Option<i64>,
) -> Result<(), crate::SpallCliError> {
    let history = crate::history::History::open(cache_dir)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;

    let full = match entry_id {
        Some(id) => history
            .get(id)
            .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?
            .ok_or_else(|| crate::SpallCliError::Usage(format!("No history entry with ID {}", id)))?,
        None => {
            let rows = history
                .list(1)
                .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
            let row = rows
                .into_iter()
                .next()
                .ok_or_else(|| crate::SpallCliError::Usage("No history to replay.".to_string()))?;
            history
                .get(row.id)
                .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?
                .expect("row should exist")
        }
    };

    let row = &full.row;
    eprintln!("Replaying request #{}: {} {}", row.id, row.method, row.url);

    // Build HTTP client using defaults.
    let client = crate::http::build_fetch_client(crate::http::resolve_env_proxy().as_deref())
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;

    let method = match row.method.to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        other => other
            .parse()
            .map_err(|_| crate::SpallCliError::Usage(format!("Unknown HTTP method: {}", other)))?,
    };

    let mut headers = HeaderMap::new();
    for (k, v) in &full.request_headers {
        if let Ok(name) = HeaderName::from_bytes(k.as_bytes()) {
            if let Ok(value) = HeaderValue::from_str(v) {
                headers.insert(name, value);
            }
        }
    }

    // Remove headers we want to regenerate.
    headers.remove(reqwest::header::CONTENT_LENGTH);

    let start = Instant::now();
    let resp = client
        .request(method, &row.url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;

    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let body = resp
        .bytes()
        .await
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;
    let duration = start.elapsed();

    let mode = crate::output::OutputMode::default();
    crate::output::emit_response(&body, mode, None)
        .map_err(|e| crate::SpallCliError::HttpClient(e.to_string()))?;

    if crate::output::OutputMode::default() == crate::output::OutputMode::Pretty {
        println!();
    }

    eprintln!("HTTP {} — {} bytes in {:?}", status, body.len(), duration);

    if status.is_client_error() {
        return Err(crate::SpallCliError::Http4xx(status.as_u16()));
    } else if status.is_server_error() {
        return Err(crate::SpallCliError::Http5xx(status.as_u16()));
    }

    Ok(())
}
