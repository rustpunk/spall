use secrecy::SecretString;

/// Credential resolution stack for an API.
///
/// Priority (highest to lowest):
/// 1. `--spall-auth` CLI pass-through (Wave 1)
/// 2. Environment variable `SPALL_\u003cAPI\u003e_TOKEN` (hyphens → underscores)
/// 3. OS keyring (Wave 3)
/// 4. Config file reference (Wave 3)
///
/// All resolved credentials are wrapped in `SecretString`.
#[derive(Debug, Clone)]
pub struct CredentialResolver {
    pub api_name: String,
}

impl CredentialResolver {
    /// Resolve the best available credential for this API.
    #[must_use]
    pub fn resolve(&self, cli_auth: Option<&str>) -> Option<Credential> {
        if let Some(raw) = cli_auth {
            return Some(parse_cli_auth(raw));
        }

        let env_name = self.env_var_name();
        if let Ok(token) = std::env::var(&env_name) {
            if !token.is_empty() {
                return Some(infer_auth(&token));
            }
        }

        None
    }

    /// Build the environment variable name for an API token.
    ///
    /// Hyphens become underscores, and the name is uppercased.
    pub fn env_var_name(&self) -> String {
        format!(
            "SPALL_{}_TOKEN",
            self.api_name.to_uppercase().replace('-', "_")
        )
    }
}

/// A resolved credential with its type.
#[derive(Debug, Clone)]
pub struct Credential {
    pub kind: CredentialKind,
    pub value: SecretString,
}

/// Kind of credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Bearer,
    Basic,
    ApiKey,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_cli_auth(raw: &str) -> Credential {
    if let Some(token) = raw.strip_prefix("Bearer ") {
        return Credential {
            kind: CredentialKind::Bearer,
            value: SecretString::new(token.to_string().into()),
        };
    }

    if let Some(token) = raw.strip_prefix("Basic ") {
        if let Ok(decoded) = base64_decode(token) {
            if let Some((u, p)) = decoded.split_once(':') {
                return Credential {
                    kind: CredentialKind::Basic,
                    value: SecretString::new(format!("{}:{}", u, p).into()),
                };
            }
        }
        if let Some((u, p)) = token.split_once(':') {
            return Credential {
                kind: CredentialKind::Basic,
                value: SecretString::new(format!("{}:{}", u, p).into()),
            };
        }
    }

    if !raw.contains(' ') && raw.split(':').count() == 2 {
        if let Some((u, p)) = raw.split_once(':') {
            return Credential {
                kind: CredentialKind::Basic,
                value: SecretString::new(format!("{}:{}", u, p).into()),
            };
        }
    }

    Credential {
        kind: CredentialKind::Bearer,
        value: SecretString::new(raw.to_string().into()),
    }
}

fn infer_auth(token: &str) -> Credential {
    if let Some((u, p)) = token.split_once(':') {
        if !u.is_empty() && !p.is_empty() && !token.contains(' ') {
            return Credential {
                kind: CredentialKind::Basic,
                value: SecretString::new(format!("{}:{}", u, p).into()),
            };
        }
    }

    Credential {
        kind: CredentialKind::Bearer,
        value: SecretString::new(token.to_string().into()),
    }
}

fn base64_decode(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let decoded = STANDARD.decode(input)?;
    Ok(String::from_utf8(decoded)?)
}
