//! API-key request contributor.

use crate::request::HttpRequestSpec;
use secrecy::{ExposeSecret, SecretString};

/// Where an API key is carried in a request.
///
/// This is spall-openapi's **own** neutral location enum, deliberately distinct
/// from `spall_config::auth::ApiKeyLocation`: this crate depends on no config
/// types. The CLI maps its resolved location into this one at the #28
/// delegation boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyLocation {
    /// Carry the key in a request header with the given name. The name is
    /// lowercased when inserted to honor the [`Headers`](crate::request::Headers)
    /// contract.
    Header {
        /// The header name (lowercased on insert).
        name: String,
    },
    /// Carry the key as a query parameter with the given name.
    Query {
        /// The query parameter name.
        name: String,
    },
}

/// Inject an API key into `req` at the configured [`ApiKeyLocation`].
///
/// * [`ApiKeyLocation::Header`] inserts the key under the lowercased header
///   name into [`HttpRequestSpec::headers`].
/// * [`ApiKeyLocation::Query`] pushes a `(name, key)` pair onto
///   [`HttpRequestSpec::query`].
///
/// Unlike the CLI's `auth::apikey::apply`, there is **no** `HeaderName` /
/// `HeaderValue` validation here: the neutral [`Headers`](crate::request::Headers)
/// map is `String`-keyed, so any name or value is storable verbatim. The CLI's
/// `x-api-key` / `invalid` fallbacks are a reqwest-construction concern and move
/// to the transport boundary in #28; this contributor stores the values as-is
/// and does not replicate that validation.
pub fn api_key(key: &SecretString, location: &ApiKeyLocation, req: &mut HttpRequestSpec) {
    match location {
        ApiKeyLocation::Header { name } => {
            // SECURITY: header-construction boundary — expose only to write the
            // key into the in-memory request spec.
            req.headers
                .insert(name.to_ascii_lowercase(), key.expose_secret().to_string());
        }
        ApiKeyLocation::Query { name } => {
            // SECURITY: query-string construction boundary.
            req.query
                .push((name.clone(), key.expose_secret().to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::empty_spec;

    #[test]
    fn header_location_lowercases_name_and_sets_value() {
        let mut req = empty_spec();
        let loc = ApiKeyLocation::Header {
            name: "X-Api-Key".to_string(),
        };
        api_key(&SecretString::new("k3y".into()), &loc, &mut req);
        // Name is lowercased to honor the Headers contract; value verbatim.
        assert_eq!(
            req.headers.get("x-api-key").map(String::as_str),
            Some("k3y")
        );
        assert!(!req.headers.contains_key("X-Api-Key"));
    }

    #[test]
    fn query_location_pushes_name_key_pair() {
        let mut req = empty_spec();
        let loc = ApiKeyLocation::Query {
            name: "api_key".to_string(),
        };
        api_key(&SecretString::new("k3y".into()), &loc, &mut req);
        assert_eq!(req.query, vec![("api_key".to_string(), "k3y".to_string())]);
    }

    #[test]
    fn key_debug_does_not_leak() {
        let key = SecretString::new("k3y".into());
        assert!(!format!("{key:?}").contains("k3y"));
    }
}
