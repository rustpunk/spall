use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use secrecy::{ExposeSecret, SecretString};

/// Inject `Authorization: Bearer <token>`.
pub fn apply(token: &SecretString, headers: &mut HeaderMap) {
    // SECURITY: header-construction boundary; do not relocate `expose_secret`
    // past a logging or serialization site.
    let value = format!("Bearer {}", token.expose_secret());
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
}
