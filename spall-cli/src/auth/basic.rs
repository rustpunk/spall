use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use secrecy::{ExposeSecret, SecretString};

/// Base64-encode `user:pass` and inject `Authorization: Basic <credentials>`.
pub fn apply(credentials: &SecretString, headers: &mut HeaderMap) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    // SECURITY: header-construction boundary.
    let encoded = STANDARD.encode(credentials.expose_secret());
    let value = format!("Basic {}", encoded);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
}
