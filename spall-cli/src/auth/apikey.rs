use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use spall_config::auth::ApiKeyLocation;

/// Runtime API key configuration.
#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    pub key: SecretString,
    pub location: ApiKeyLocation,
}

/// Inject an API key into a header or query string.
pub fn apply(
    config: &ApiKeyConfig,
    headers: &mut HeaderMap,
    query_pairs: &mut Vec<(String, String)>,
) {
    match &config.location {
        ApiKeyLocation::Header { name } => {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .unwrap_or_else(|_| HeaderName::from_static("x-api-key")),
                HeaderValue::from_str(config.key.expose_secret())
                    .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
            );
        }
        ApiKeyLocation::Query { name } => {
            query_pairs.push((name.clone(), config.key.expose_secret().to_string()));
        }
    }
}
