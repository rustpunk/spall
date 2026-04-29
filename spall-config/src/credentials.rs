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
    pub fn resolve(&self, _cli_auth: Option<&str>) -> Option<Credential> {
        // TODO(Wave 1): check env vars. Wave 3: keyring, config.
        todo!("CredentialResolver::resolve")
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
