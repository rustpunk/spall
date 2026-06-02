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
///
/// # Classification precedence
///
/// A raw auth value is classified into a [`CredentialKind`] by the following
/// ladder, highest priority to lowest:
///
/// 1. Explicit `Bearer <token>` prefix → [`CredentialKind::Bearer`].
/// 2. Explicit `Basic <base64-or-user:pass>` prefix → [`CredentialKind::Basic`].
/// 3. Unambiguous `user:pass` shape → [`CredentialKind::Basic`]. A token is
///    unambiguous only when it has exactly one colon, contains no ASCII
///    whitespace, both halves are non-empty, and the substring after the colon
///    does not start with `//` (the last clause rejects every `scheme://...`
///    URL, e.g. `https://`, `keyring://`, `env://`).
/// 4. Otherwise → [`CredentialKind::Bearer`] (the safe default).
///
/// This ladder is identical for the `--spall-auth` CLI flag and the
/// `SPALL_<API>_TOKEN` environment variable, **except** that the env path never
/// applies the explicit-prefix steps (1) and (2): prefixes are not stripped from
/// env tokens (matching prior behavior), so an env token is always classified by
/// steps (3) and (4) only.
///
/// Steps (3) and (4) are the shared classifier `classify_bare_token`; both the
/// CLI bare-token fallthrough and the env path route through it, so a
/// byte-identical token classifies identically regardless of source.
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

    bare_credential(raw)
}

fn infer_auth(token: &str) -> Credential {
    bare_credential(token)
}

/// Sole authority for the Basic-vs-Bearer decision on a prefix-less token.
///
/// Returns [`CredentialKind::Basic`] only when `token` is an unambiguous
/// `user:pass`: no ASCII whitespace, exactly one colon, both halves non-empty,
/// and the substring after the colon not starting with `//` (which rejects every
/// `scheme://...` URL). Every other token is [`CredentialKind::Bearer`], the safe
/// default. See [`Credential`] for the full precedence ladder.
fn classify_bare_token(token: &str) -> CredentialKind {
    if token.contains(|c: char| c.is_ascii_whitespace()) {
        return CredentialKind::Bearer;
    }
    if token.split(':').count() != 2 {
        return CredentialKind::Bearer;
    }
    match token.split_once(':') {
        Some((u, p)) if !u.is_empty() && !p.is_empty() && !p.starts_with("//") => {
            CredentialKind::Basic
        }
        _ => CredentialKind::Bearer,
    }
}

/// Build a [`Credential`] from a prefix-less token using [`classify_bare_token`].
///
/// For the Basic arm the value is normalized to `format!("{u}:{p}")`, identical
/// to the prior inline behavior; for Bearer the raw token is wrapped verbatim.
fn bare_credential(token: &str) -> Credential {
    match classify_bare_token(token) {
        CredentialKind::Basic => {
            let (u, p) = token.split_once(':').unwrap_or((token, ""));
            Credential {
                kind: CredentialKind::Basic,
                value: SecretString::new(format!("{}:{}", u, p).into()),
            }
        }
        _ => Credential {
            kind: CredentialKind::Bearer,
            value: SecretString::new(token.to_string().into()),
        },
    }
}

fn base64_decode(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let decoded = STANDARD.decode(input)?;
    Ok(String::from_utf8(decoded)?)
}
