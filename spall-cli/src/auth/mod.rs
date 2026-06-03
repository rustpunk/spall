//! Auth resolution and request injection.

pub mod oauth2;

use secrecy::{ExposeSecret, SecretString};
use spall_config::auth::{ApiKeyLocation, AuthConfig, AuthKind, ResolvedAuth};
use spall_config::credentials::CredentialKind;
use spall_openapi::HttpRequestSpec;

/// Resolve authentication material following the priority chain:
///
/// 1. `--spall-auth` CLI override
/// 2. `auth.token` inline in per-API TOML (warn on use)
/// 3. `auth.token_url` (`keyring://` / `env://`) — requires `hasp` feature
/// 4. `auth.password_url` for Basic auth — requires `hasp` feature
/// 5. `auth.token_env` env var override
/// 6. Global `SPALL_<API>_TOKEN` env var (Wave 1–2 compat)
/// 7. OAuth2 interactive flow — session token only (stub)
/// 8. Interactive password prompt for Basic
///
/// SECURITY note (per #13): credential ingresses (clap arg, env var,
/// TOML field) wrap into `SecretString` within the same statement that
/// crosses the FFI/OS boundary; see `deserialize_secret_string`.
pub async fn resolve(
    api_name: &str,
    auth_config: Option<&AuthConfig>,
    cli_auth: Option<&str>,
) -> Result<Option<ResolvedAuth>, crate::SpallCliError> {
    // 1. CLI override.
    if let Some(raw) = cli_auth {
        return Ok(Some(parse_cli_auth(raw)));
    }

    let cfg = match auth_config {
        Some(c) => c,
        None => return Ok(None),
    };
    let kind = cfg.kind.unwrap_or(AuthKind::Bearer);

    // 2. Inline token (warn on use).
    if let Some(token) = &cfg.token {
        eprintln!(
            "Warning: inline auth token in config is insecure. Use an env var or keyring instead."
        );
        // SECURITY: header-construction boundary.
        return Ok(resolve_from_config_and_token(
            cfg,
            kind,
            token.expose_secret(),
        ));
    }

    // 3. token_url (keyring:// / env://) — requires `hasp`.
    //
    // Skipped for `kind = oauth2`: for OAuth2 the `token_url` field is the
    // IDP's token endpoint (an http(s) URL), not a hasp reference. Stored
    // OAuth2 tokens live in spall's own cache dir, handled at step 8.
    #[cfg(feature = "hasp")]
    if kind != AuthKind::OAuth2 {
        if let Some(url) = &cfg.token_url {
            let secret = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
            // SECURITY: header-construction boundary.
            return Ok(resolve_from_config_and_token(
                cfg,
                kind,
                secret.expose_secret(),
            ));
        }
    }

    // 4. Basic password_url takes precedence over password_env.
    #[cfg(feature = "hasp")]
    if kind == AuthKind::Basic {
        if let Some(url) = &cfg.password_url {
            let password = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
            if let Some(username) = &cfg.username {
                return Ok(Some(ResolvedAuth::Basic {
                    username: username.clone(),
                    password,
                }));
            }
        }
    }

    // 5. Basic password_env takes precedence over generic token_env.
    if kind == AuthKind::Basic {
        if let Some(env_name) = &cfg.password_env {
            if let Ok(password) = std::env::var(env_name) {
                if !password.is_empty() {
                    if let Some(username) = &cfg.username {
                        return Ok(Some(ResolvedAuth::Basic {
                            username: username.clone(),
                            password: SecretString::new(password.into()),
                        }));
                    }
                }
            }
        }
    }

    // 6. token_env.
    if let Some(env_name) = &cfg.token_env {
        if let Ok(token) = std::env::var(env_name) {
            if !token.is_empty() {
                return Ok(resolve_from_config_and_token(cfg, kind, &token));
            }
        }
    }

    // 7. Global SPALL_<API>_TOKEN.
    let default_env = spall_config::auth::default_token_env(api_name);
    if let Ok(token) = std::env::var(&default_env) {
        if !token.is_empty() {
            return Ok(resolve_from_config_and_token(cfg, kind, &token));
        }
    }

    // 8. OAuth2 stored tokens from `spall auth login`. Refresh transparently
    //    when the cached access token is past (or within 30s of) its expiry.
    if kind == AuthKind::OAuth2 {
        let stored = oauth2::ensure_fresh_token(api_name).await?;
        return Ok(stored.map(ResolvedAuth::OAuth2));
    }

    // 9. Interactive password prompt for Basic.
    if kind == AuthKind::Basic {
        if let Some(username) = &cfg.username {
            eprint!("Password for {} (Basic auth): ", username);
            if let Ok(password) = rpassword::read_password() {
                return Ok(Some(ResolvedAuth::Basic {
                    username: username.clone(),
                    password: SecretString::new(password.into()),
                }));
            }
        }
    }

    Ok(None)
}

/// Apply a resolved auth value to a transport-neutral request spec by
/// dispatching to the matching `spall_openapi::auth` contributor.
///
/// The reqwest-specific header/query injection that used to live in
/// `auth/{bearer,basic,apikey,oauth2}.rs` now lives once, in `spall-openapi`;
/// this function only translates `ResolvedAuth` into a contributor call and
/// maps `spall_config`'s `ApiKeyLocation` into spall-openapi's neutral one.
pub fn apply(auth: &ResolvedAuth, spec: &mut HttpRequestSpec) {
    match auth {
        ResolvedAuth::Bearer(token) => spall_openapi::bearer(token, spec),
        ResolvedAuth::ApiKey { key, location } => {
            spall_openapi::api_key(key, &map_api_key_location(location), spec);
        }
        ResolvedAuth::Basic { username, password } => {
            // SECURITY: credential-join boundary — spall_openapi::basic
            // base64-encodes the joined `user:pass`.
            let creds = format!("{}:{}", username, password.expose_secret());
            spall_openapi::basic(&SecretString::new(creds.into()), spec);
        }
        ResolvedAuth::OAuth2(token) => spall_openapi::oauth2_access_token(token, spec),
    }
}

/// Map `spall_config`'s `ApiKeyLocation` into spall-openapi's neutral one.
fn map_api_key_location(location: &ApiKeyLocation) -> spall_openapi::ApiKeyLocation {
    match location {
        ApiKeyLocation::Header { name } => {
            spall_openapi::ApiKeyLocation::Header { name: name.clone() }
        }
        ApiKeyLocation::Query { name } => {
            spall_openapi::ApiKeyLocation::Query { name: name.clone() }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `hasp::Error` into a `SpallCliError`.
#[cfg(feature = "hasp")]
fn map_hasp_error(api_name: &str, e: hasp::Error) -> crate::SpallCliError {
    crate::SpallCliError::AuthResolution {
        api: api_name.to_string(),
        message: e.to_string(),
    }
}

fn parse_cli_auth(raw: &str) -> ResolvedAuth {
    if let Some(token) = raw.strip_prefix("Bearer ") {
        return ResolvedAuth::Bearer(SecretString::new(token.to_string().into()));
    }

    if let Some(token) = raw.strip_prefix("Basic ") {
        // Try base64-decode first.
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        if let Ok(decoded) = STANDARD.decode(token) {
            if let Ok(decoded_str) = String::from_utf8(decoded) {
                if let Some((u, p)) = decoded_str.split_once(':') {
                    return ResolvedAuth::Basic {
                        username: u.to_string(),
                        password: SecretString::new(p.to_string().into()),
                    };
                }
            }
        }
        // Fall back to literal user:pass.
        if let Some((u, p)) = token.split_once(':') {
            return ResolvedAuth::Basic {
                username: u.to_string(),
                password: SecretString::new(p.to_string().into()),
            };
        }
    }

    // Bare token (no explicit prefix): defer the Basic-vs-Bearer decision to the
    // single shared classifier in spall-config, so this rule can never drift
    // from other credential consumers. Basic is chosen only for an unambiguous
    // `user:pass` (one colon, no whitespace, non-empty halves, not a scheme://
    // URL); everything else — including `https://host` — is Bearer.
    match spall_config::credentials::classify_bare_token(raw) {
        CredentialKind::Basic => {
            // classify_bare_token guarantees a single colon with non-empty halves.
            let (u, p) = raw.split_once(':').unwrap_or((raw, ""));
            ResolvedAuth::Basic {
                username: u.to_string(),
                password: SecretString::new(p.to_string().into()),
            }
        }
        _ => ResolvedAuth::Bearer(SecretString::new(raw.to_string().into())),
    }
}

fn resolve_from_config_and_token(
    cfg: &AuthConfig,
    kind: AuthKind,
    token: &str,
) -> Option<ResolvedAuth> {
    match kind {
        AuthKind::Bearer => Some(ResolvedAuth::Bearer(SecretString::new(
            token.to_string().into(),
        ))),
        AuthKind::ApiKey => {
            let location = match cfg.location.as_deref() {
                Some("query") => ApiKeyLocation::Query {
                    name: cfg
                        .query_name
                        .clone()
                        .unwrap_or_else(|| "api_key".to_string()),
                },
                _ => ApiKeyLocation::Header {
                    name: cfg
                        .header_name
                        .clone()
                        .unwrap_or_else(|| "X-Api-Key".to_string()),
                },
            };
            Some(ResolvedAuth::ApiKey {
                key: SecretString::new(token.to_string().into()),
                location,
            })
        }
        AuthKind::Basic => {
            // If config has username, token is the password (or legacy user:pass).
            if let Some(username) = &cfg.username {
                let password = if let Some((_, p)) = token.split_once(':') {
                    p.to_string()
                } else {
                    token.to_string()
                };
                return Some(ResolvedAuth::Basic {
                    username: username.clone(),
                    password: SecretString::new(password.into()),
                });
            }
            // No username in config; try splitting token as user:pass.
            if let Some((u, p)) = token.split_once(':') {
                Some(ResolvedAuth::Basic {
                    username: u.to_string(),
                    password: SecretString::new(p.to_string().into()),
                })
            } else {
                // Can't resolve Basic without username — fall back to Bearer.
                Some(ResolvedAuth::Bearer(SecretString::new(
                    token.to_string().into(),
                )))
            }
        }
        AuthKind::OAuth2 => Some(ResolvedAuth::OAuth2(SecretString::new(
            token.to_string().into(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cli_override_bearer() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), Some("Bearer abc123"))
            .await
            .unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "abc123"
        ));
    }

    #[tokio::test]
    async fn cli_override_basic_shorthand() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Basic),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), Some("alice:secret"))
            .await
            .unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Basic { ref username, ref password })
            if username == "alice" && password.expose_secret() == "secret"
        ));
    }

    // #38: a `--spall-auth` value that merely looks like `user:pass` because it
    // contains one colon must not be misread as Basic. A `scheme://...` URL is
    // the canonical trap (one colon, no space) and must classify as Bearer.
    #[tokio::test]
    async fn cli_override_url_shaped_is_bearer() {
        let result = resolve("test", None, Some("https://example.com"))
            .await
            .unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "https://example.com"
        ));
    }

    #[tokio::test]
    async fn cli_override_multi_colon_is_bearer() {
        let result = resolve("test", None, Some("a:b:c")).await.unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "a:b:c"
        ));
    }

    #[tokio::test]
    async fn cli_override_empty_half_is_bearer() {
        // An empty username or password half is not valid Basic; fall to Bearer.
        assert!(matches!(
            resolve("test", None, Some(":secret")).await.unwrap(),
            Some(ResolvedAuth::Bearer(_))
        ));
        assert!(matches!(
            resolve("test", None, Some("user:")).await.unwrap(),
            Some(ResolvedAuth::Bearer(_))
        ));
    }

    #[tokio::test]
    async fn inline_token_warns_and_resolves() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            token: Some(SecretString::new("tkn".to_string().into())),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), None).await.unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "tkn"
        ));
    }

    #[tokio::test]
    async fn token_env_resolves() {
        let var = "SPALL_AUTH_TEST_TOKEN";
        std::env::set_var(var, "env-tkn");
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            token_env: Some(var.to_string()),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), None).await.unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "env-tkn"
        ));
        std::env::remove_var(var);
    }

    #[tokio::test]
    async fn no_auth_config_returns_none() {
        let result = resolve("test", None, None).await.unwrap();
        assert!(result.is_none());
    }

    #[cfg(feature = "hasp")]
    #[test]
    fn map_hasp_error_produces_auth_resolution() {
        let err = hasp::Error::NotFound("env://MISSING".to_string());
        let mapped = super::map_hasp_error("github", err);
        let msg = format!("{}", mapped);
        assert!(msg.contains("github"));
        assert!(msg.contains("not found"));
    }
}
