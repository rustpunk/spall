//! Auth resolution and request injection.

pub mod apikey;
pub mod basic;
pub mod bearer;
pub mod oauth2;

use reqwest::header::HeaderMap;
use secrecy::{ExposeSecret, SecretString};
use spall_config::auth::{ApiKeyLocation, AuthConfig, AuthKind, ResolvedAuth};

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
pub fn resolve(
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
        eprintln!("Warning: inline auth token in config is insecure. Use an env var or keyring instead.");
        return Ok(resolve_from_config_and_token(cfg, kind, token));
    }

    // 3. token_url (keyring:// / env://) — requires `hasp`.
    #[cfg(feature = "hasp")]
    if let Some(url) = &cfg.token_url {
        let secret = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
        return Ok(resolve_from_config_and_token(cfg, kind, secret.expose_secret()));
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

    // 8. OAuth2 client_secret_url (future-proofing; full PKCE flow not yet implemented).
    #[cfg(feature = "hasp")]
    if kind == AuthKind::OAuth2 {
        if let Some(_url) = &cfg.client_secret_url {
            // TODO(Wave 3+): integrate into full OAuth2 PKCE flow.
        }
    }

    // 9. OAuth2 session token (stub — no session persistence yet).
    if kind == AuthKind::OAuth2 {
        return Ok(None);
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

/// Apply a resolved auth value to the request.
pub fn apply(
    auth: &ResolvedAuth,
    headers: &mut HeaderMap,
    query_pairs: &mut Vec<(String, String)>,
) {
    match auth {
        ResolvedAuth::Bearer(token) => bearer::apply(token, headers),
        ResolvedAuth::ApiKey { key, location } => {
            apikey::apply(
                &apikey::ApiKeyConfig { key: key.clone(), location: location.clone() },
                headers,
                query_pairs,
            );
        }
        ResolvedAuth::Basic { username, password } => {
            let creds = format!("{}:{}", username, password.expose_secret());
            basic::apply(&SecretString::new(creds.into()), headers);
        }
        ResolvedAuth::OAuth2(token) => oauth2::apply(token, headers),
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

    // User:pass shorthand (no space, contains exactly one colon).
    if !raw.contains(' ') && raw.split(':').count() == 2 {
        let (u, p) = raw.split_once(':').unwrap();
        return ResolvedAuth::Basic {
            username: u.to_string(),
            password: SecretString::new(p.to_string().into()),
        };
    }

    // Bare token → Bearer.
    ResolvedAuth::Bearer(SecretString::new(raw.to_string().into()))
}

fn resolve_from_config_and_token(
    cfg: &AuthConfig,
    kind: AuthKind,
    token: &str,
) -> Option<ResolvedAuth> {
    match kind {
        AuthKind::Bearer => Some(ResolvedAuth::Bearer(SecretString::new(token.to_string().into()))),
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
                Some(ResolvedAuth::Bearer(SecretString::new(token.to_string().into())))
            }
        }
        AuthKind::OAuth2 => {
            Some(ResolvedAuth::OAuth2(SecretString::new(token.to_string().into())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_override_bearer() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), Some("Bearer abc123")).unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "abc123"
        ));
    }

    #[test]
    fn cli_override_basic_shorthand() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Basic),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), Some("alice:secret")).unwrap();
        assert!(
            matches!(
                result,
                Some(ResolvedAuth::Basic { ref username, ref password })
                if username == "alice" && password.expose_secret() == "secret"
            )
        );
    }

    #[test]
    fn inline_token_warns_and_resolves() {
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            token: Some("tkn".to_string()),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), None).unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "tkn"
        ));
    }

    #[test]
    fn token_env_resolves() {
        let var = "SPALL_AUTH_TEST_TOKEN";
        std::env::set_var(var, "env-tkn");
        let cfg = AuthConfig {
            kind: Some(AuthKind::Bearer),
            token_env: Some(var.to_string()),
            ..Default::default()
        };
        let result = resolve("test", Some(&cfg), None).unwrap();
        assert!(matches!(
            result,
            Some(ResolvedAuth::Bearer(ref s)) if s.expose_secret() == "env-tkn"
        ));
        std::env::remove_var(var);
    }

    #[test]
    fn no_auth_config_returns_none() {
        let result = resolve("test", None, None).unwrap();
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
