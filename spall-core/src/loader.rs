use crate::error::SpallCoreError;
use crate::ir::ResolvedSpec;
use crate::resolver::resolve_spec;

/// Load and resolve an OpenAPI spec from a file path or URL.
///
/// Wave 1: supports file paths. URL loading is deferred to Wave 1.5.
pub fn load_spec(source: &str) -> Result<ResolvedSpec, SpallCoreError> {
    let raw = load_raw(source)?;
    load_spec_from_bytes(&raw, source)
}

/// Parse and resolve raw spec bytes into a `ResolvedSpec`.
pub fn load_spec_from_bytes(raw: &[u8], source: &str) -> Result<ResolvedSpec, SpallCoreError> {
    let text = String::from_utf8_lossy(raw);

    let openapi: openapiv3::OpenAPI = if looks_like_json(&text) {
        serde_json::from_str(&text).map_err(|e| SpallCoreError::SpecParse {
            message: e.to_string(),
            url: source.to_string(),
        })?
    } else {
        match serde_saphyr::from_str::<openapiv3::OpenAPI>(&text) {
            Ok(o) => o,
            Err(e) => {
                // JSON fallback: many "YAML" URLs actually serve JSON.
                if let Ok(o) = serde_json::from_str::<openapiv3::OpenAPI>(&text) {
                    o
                } else {
                    return Err(SpallCoreError::SpecParse {
                        message: e.to_string(),
                        url: source.to_string(),
                    });
                }
            }
        }
    };

    resolve_spec(&openapi, source)
}

/// Load raw spec bytes from a source.
///
/// Wave 1: local file system only. Wave 1.5: URL fetching with caching.
pub fn load_raw(source: &str) -> Result<Vec<u8>, SpallCoreError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return Err(SpallCoreError::InvalidSource(
            format!("URL sources not yet supported in Wave 1: {}", source),
        ));
    }

    let path = std::path::PathBuf::from(source);
    std::fs::read(&path).map_err(|e| SpallCoreError::Io(e.to_string()))
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}
