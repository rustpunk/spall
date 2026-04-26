//! RFC 8631 spec autodiscovery.

use reqwest::header::LINK;

/// Probe a URL for an RFC 8631 `service-desc` Link header, fetch the spec,
/// and return a suggested API name + spec URL.
pub async fn probe(url: &str) -> Result<DiscoveredApi, crate::SpallCliError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;

    // 1. HEAD first, fall back to GET.
    let resp = match client.head(url).send().await {
        Ok(r) => r,
        Err(_) => client
            .get(url)
            .send()
            .await
            .map_err(|e| crate::SpallCliError::Network(e.to_string()))?,
    };

    // 2. Extract Link header.
    let spec_url = if let Some(link_hdr) = resp.headers().get(LINK) {
        parse_service_desc_link(link_hdr.to_str().unwrap_or(""))
            .or_else(|| resolve_absolute_url(url, guess_well_known(url)))
    } else {
        resolve_absolute_url(url, guess_well_known(url))
    };

    let spec_url = spec_url.ok_or_else(|| {
        crate::SpallCliError::Usage(format!(
            "Could not discover spec from {}. No Link: rel=service-desc header found and no well-known path guessed.",
            url
        ))
    })?;

    // 3. Fetch the spec to extract a title.
    let spec_resp = client
        .get(&spec_url)
        .send()
        .await
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;

    if !spec_resp.status().is_success() {
        return Err(crate::SpallCliError::Network(format!(
            "Discovered spec URL returned HTTP {}",
            spec_resp.status()
        )));
    }

    let spec_bytes = spec_resp
        .bytes()
        .await
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;

    let title = extract_title(&spec_bytes).unwrap_or_else(|| default_name_from_url(url));
    let name = slugify(&title);

    Ok(DiscoveredApi {
        name,
        title,
        spec_url,
    })
}

/// A discovered API.
#[derive(Debug, Clone)]
pub struct DiscoveredApi {
    pub name: String,
    pub title: String,
    pub spec_url: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_service_desc_link(header: &str) -> Option<String> {
    // Link: <https://api.example.com/openapi.json>; rel="service-desc"
    // May contain multiple comma-separated links.
    for part in header.split(',') {
        let trimmed = part.trim();
        if trimmed.contains(r#"rel="service-desc"#)
            || trimmed.contains("rel=\"service-desc\"")
            || trimmed.contains("rel='service-desc'")
            || trimmed.contains("rel=service-desc")
        {
            if let Some(start) = trimmed.find('<') {
                if let Some(end) = trimmed[start..].find('>') {
                    return Some(trimmed[start + 1..start + end].to_string());
                }
            }
            // Try quoted URL
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed[start + 1..].find('"') {
                    return Some(trimmed[start + 1..start + 1 + end].to_string());
                }
            }
        }
    }
    None
}

fn guess_well_known(base: &str) -> Option<String> {
    let well_known = [
        "/openapi.json",
        "/openapi.yaml",
        "/swagger.json",
        "/swagger.yaml",
        "/api/openapi.json",
        "/.well-known/openapi",
    ];
    for path in &well_known {
        if let Ok(base_url) = reqwest::Url::parse(base) {
            if let Ok(merged) = base_url.join(path) {
                return Some(merged.to_string());
            }
        }
    }
    None
}

fn resolve_absolute_url(base: &str, relative: Option<String>) -> Option<String> {
    let rel = relative?;
    if rel.starts_with("http://") || rel.starts_with("https://") {
        Some(rel)
    } else {
        reqwest::Url::parse(base)
            .ok()
            .and_then(|b| b.join(&rel).ok())
            .map(|u| u.to_string())
    }
}

fn extract_title(spec_bytes: &[u8]) -> Option<String> {
    // Fast path: try JSON.
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(spec_bytes) {
        if let Some(title) = json.get("info")?.get("title")?.as_str() {
            return Some(title.to_string());
        }
    }
    // YAML fallback — try simple regex-like scan for "title:".
    let text = String::from_utf8_lossy(spec_bytes);
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("title:") {
            let rest = rest.trim();
            let rest = rest.strip_prefix('"').unwrap_or(rest);
            let rest = rest.strip_suffix('"').unwrap_or(rest);
            let rest = rest.strip_prefix("'").unwrap_or(rest);
            let rest = rest.strip_suffix("'").unwrap_or(rest);
            return Some(rest.to_string());
        }
    }
    None
}

fn default_name_from_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("discovered")
        .split(':')
        .next()
        .unwrap_or("discovered")
        .to_string()
}

fn slugify(s: &str) -> String {
    s.to_ascii_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
        .replace("__", "-")
        .replace("--", "-")
        .trim_matches('-')
        .to_string()
}
