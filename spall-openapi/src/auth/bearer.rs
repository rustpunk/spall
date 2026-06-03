//! Bearer-token request contributor.

use crate::request::HttpRequestSpec;
use secrecy::{ExposeSecret, SecretString};

/// Inject `authorization: Bearer <token>` into `req`.
///
/// Mirrors the CLI's `auth::bearer::apply`, but writes into the neutral
/// lowercased [`Headers`](crate::request::Headers) map: the key is the literal
/// `"authorization"`. The token is taken already resolved; this crate never
/// decides *which* token to use.
pub fn bearer(token: &SecretString, req: &mut HttpRequestSpec) {
    // SECURITY: header-construction boundary — the only place the secret is
    // exposed, and only into the in-memory request spec (never an IR type).
    let value = format!("Bearer {}", token.expose_secret());
    req.headers.insert("authorization".to_string(), value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::empty_spec;

    #[test]
    fn sets_authorization_bearer() {
        let mut req = empty_spec();
        bearer(&SecretString::new("abc123".into()), &mut req);
        assert_eq!(
            req.headers.get("authorization").map(String::as_str),
            Some("Bearer abc123")
        );
    }

    #[test]
    fn token_debug_does_not_leak() {
        // The contributor input is a SecretString, which redacts under Debug.
        let token = SecretString::new("super-secret".into());
        assert!(!format!("{token:?}").contains("super-secret"));
    }
}
