//! OAuth2 request contributors: access-token injection and the
//! client-credentials token-request spec.
//!
//! The rich OAuth2 surface — Authorization-Code + PKCE login, the on-disk token
//! cache, and transparent refresh — stays in `spall-cli`. This module carries
//! only the two transport-neutral pieces: injecting an already-acquired access
//! token (identical to [`bearer`]) and constructing the client-credentials
//! token *request* as an [`HttpRequestSpec`] the caller executes itself.

use crate::auth::bearer::bearer;
use crate::request::{Headers, HttpRequestSpec, RequestBody};
use secrecy::{ExposeSecret, SecretString};

/// Inject an OAuth2 access token as `authorization: Bearer <token>`.
///
/// The CLI's `oauth2::apply` is byte-identical to its `bearer::apply`, so this
/// delegates to [`bearer`] rather than duplicating the header logic.
pub fn oauth2_access_token(token: &SecretString, req: &mut HttpRequestSpec) {
    bearer(token, req);
}

/// Build the OAuth2 **client-credentials** token-request as a neutral
/// [`HttpRequestSpec`] the caller executes (this crate performs no HTTP I/O).
///
/// The result is a `POST` to `token_url` with an
/// `application/x-www-form-urlencoded` body whose pairs are, in order:
///
/// 1. `grant_type=client_credentials`
/// 2. `client_id=<client_id>`
/// 3. `client_secret=<client_secret>`
/// 4. `scope=<scope>` — only when `scope` is `Some`
///
/// The form content type is set here on the returned spec (this builder bypasses
/// [`build_request`](crate::build_request), which would otherwise apply it), so
/// the contract matches a form body assembled through the request builder.
///
/// This grant is not implemented in spall-cli today; #26 specifies it directly
/// as a request spec so any transport can run the client-credentials flow.
#[must_use = "the returned HttpRequestSpec is the token request to execute"]
pub fn oauth2_client_credentials_request(
    token_url: &str,
    client_id: &str,
    client_secret: &SecretString,
    scope: Option<&str>,
) -> HttpRequestSpec {
    let mut form: Vec<(String, String)> = vec![
        ("grant_type".to_string(), "client_credentials".to_string()),
        ("client_id".to_string(), client_id.to_string()),
        // SECURITY: form-body construction boundary — expose only to place the
        // client secret into the in-memory token-request body (never an IR type).
        (
            "client_secret".to_string(),
            client_secret.expose_secret().to_string(),
        ),
    ];
    if let Some(scope) = scope {
        form.push(("scope".to_string(), scope.to_string()));
    }

    let mut headers = Headers::new();
    headers.insert(
        "content-type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    );

    HttpRequestSpec {
        method: spall_core::ir::HttpMethod::Post,
        url: token_url.to_string(),
        query: Vec::new(),
        headers,
        cookies: Vec::new(),
        body: Some(RequestBody::Form(form)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::bearer::bearer;
    use crate::auth::test_support::empty_spec;

    #[test]
    fn access_token_matches_bearer_behavior() {
        let token = SecretString::new("at-123".into());

        let mut via_oauth2 = empty_spec();
        oauth2_access_token(&token, &mut via_oauth2);

        let mut via_bearer = empty_spec();
        bearer(&token, &mut via_bearer);

        assert_eq!(via_oauth2.headers, via_bearer.headers);
        assert_eq!(
            via_oauth2.headers.get("authorization").map(String::as_str),
            Some("Bearer at-123")
        );
    }

    #[test]
    fn client_credentials_request_with_scope() {
        let req = oauth2_client_credentials_request(
            "https://idp.example.com/token",
            "client-abc",
            &SecretString::new("s3cr3t".into()),
            Some("read write"),
        );

        assert_eq!(req.method, spall_core::ir::HttpMethod::Post);
        assert_eq!(req.url, "https://idp.example.com/token");
        // The form builder sets the urlencoded content type.
        assert_eq!(
            req.headers.get("content-type").map(String::as_str),
            Some("application/x-www-form-urlencoded")
        );
        match req.body {
            Some(RequestBody::Form(pairs)) => {
                assert_eq!(
                    pairs,
                    vec![
                        ("grant_type".to_string(), "client_credentials".to_string()),
                        ("client_id".to_string(), "client-abc".to_string()),
                        ("client_secret".to_string(), "s3cr3t".to_string()),
                        ("scope".to_string(), "read write".to_string()),
                    ]
                );
            }
            other => panic!("expected Form body, got {other:?}"),
        }
    }

    #[test]
    fn client_credentials_request_without_scope_omits_scope() {
        let req = oauth2_client_credentials_request(
            "https://idp.example.com/token",
            "client-abc",
            &SecretString::new("s3cr3t".into()),
            None,
        );
        match req.body {
            Some(RequestBody::Form(pairs)) => {
                assert!(
                    pairs.iter().all(|(k, _)| k != "scope"),
                    "scope must be absent when None: {pairs:?}"
                );
                assert_eq!(
                    pairs,
                    vec![
                        ("grant_type".to_string(), "client_credentials".to_string()),
                        ("client_id".to_string(), "client-abc".to_string()),
                        ("client_secret".to_string(), "s3cr3t".to_string()),
                    ]
                );
            }
            other => panic!("expected Form body, got {other:?}"),
        }
    }

    #[test]
    fn client_secret_debug_does_not_leak() {
        let secret = SecretString::new("s3cr3t".into());
        assert!(!format!("{secret:?}").contains("s3cr3t"));
    }
}
