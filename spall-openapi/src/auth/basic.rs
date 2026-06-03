//! HTTP Basic-auth request contributor.

use crate::request::HttpRequestSpec;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret, SecretString};

/// Base64-encode the already-joined `user:pass` credentials and inject
/// `authorization: Basic <encoded>` into `req`.
///
/// The caller joins `username:password` into a single `SecretString` before
/// calling — exactly as the CLI's `auth::apply` does today — so this function
/// only encodes and injects; it never sees the username/password split. The
/// encoding is base64 STANDARD over the raw credential bytes, matching
/// `auth::basic::apply`. Writes into the neutral lowercased
/// [`Headers`](crate::request::Headers) map under the literal `"authorization"`.
pub fn basic(credentials: &SecretString, req: &mut HttpRequestSpec) {
    // SECURITY: header-construction boundary — expose only to base64-encode the
    // joined credentials into the in-memory request spec.
    let encoded = STANDARD.encode(credentials.expose_secret());
    let value = format!("Basic {}", encoded);
    req.headers.insert("authorization".to_string(), value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::empty_spec;

    #[test]
    fn base64_encodes_joined_credentials() {
        let mut req = empty_spec();
        basic(&SecretString::new("user:pass".into()), &mut req);
        // base64 STANDARD("user:pass") == "dXNlcjpwYXNz" (assert exact encoding).
        assert_eq!(
            req.headers.get("authorization").map(String::as_str),
            Some("Basic dXNlcjpwYXNz")
        );
    }

    #[test]
    fn credentials_debug_does_not_leak() {
        let creds = SecretString::new("user:pass".into());
        assert!(!format!("{creds:?}").contains("user:pass"));
    }
}
