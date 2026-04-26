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
/// 3. `auth.token_url` (`keyring://` / `env://`) — stub, gated on `hasp` feature
/// 4. `auth.token_env` env var override
/// 5. Global `SPALL_<API>_TOKEN` env var (Wave 1–2 compat)
/// 6. OAuth2 interactive flow — session token only (stub)
/// 7. Interactive password prompt for Basic
#[must_use]
pub fn resolve(
    api_name: &str,
    auth_config: Option<&AuthConfig>,
    cli_auth: Option<&str>,
) -> Option<ResolvedAuth> {
    // 1. CLI override.
    if let Some(raw) = cli_auth {
        return Some(parse_cli_auth(raw));
    }

    let cfg = auth_config?;
    let kind = cfg.kind.unwrap_or(AuthKind::Bearer);

    // 2. Inline token (warn on use).
    if let Some(token) = &cfg.token {
        eprintln!("Warning: inline auth token in config is insecure. Use an env var or keyring instead.");
        return resolve_from_config_and_token(cfg, kind, token);
    }

    // 3. token_url (keyring:// / env://) — requires `hasp`.
    #[cfg(feature = "hasp")]
    if let Some(url) = &cfg.token_url {
        // TODO(hasp): resolve via hasp::get(url)
        let _ = url;
    }

    // 4. Basic password_env takes precedence over generic token_env.
    if kind == AuthKind::Basic {
        if let Some(env_name) = &cfg.password_env {
            if let Ok(password) = std::env::var(env_name) {
                if !password.is_empty() {
                    if let Some(username) = &cfg.username {
                        return Some(ResolvedAuth::Basic {
                            username: username.clone(),
                            password: SecretString::new(password.into()),
                        });
                    }
                }
            }
        }
    }

    // 5. token_env.
    if let Some(env_name) = &cfg.token_env {
        if let Ok(token) = std::env::var(env_name) {
            if !token.is_empty() {
                return resolve_from_config_and_token(cfg, kind, &token);
            }
        }
    }

    // 6. Global SPALL_<API>_TOKEN.
    let default_env = spall_config::auth::default_token_env(api_name);
    if let Ok(token) = std::env::var(&default_env) {
        if !token.is_empty() {
            return resolve_from_config_and_token(cfg, kind, &token);
        }
    }

    // 7. OAuth2 session token (stub — no session persistence yet).
    if kind == AuthKind::OAuth2 {
        return None;
    }

    // 8. Interactive password prompt for Basic.
    if kind == AuthKind::Basic {
        if let Some(username) = &cfg.username {
            eprint!("Password for {} (Basic auth): ", username);
            if let Ok(password) = rpassword::read_password() {
                return Some(ResolvedAuth::Basic {
                    username: username.clone(),
                    password: SecretString::new(password.into()),
                });
            }
        }
    }

    None
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
