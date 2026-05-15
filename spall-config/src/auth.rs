use secrecy::SecretString;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Custom deserializer for optional inline secret fields. Accepts the
/// existing TOML/JSON `field = "value"` form and wraps the raw string
/// in `SecretString` on read, so callers never see the unwrapped value
/// via `Debug` or accidental serialization.
fn deserialize_secret_string<'de, D>(d: D) -> Result<Option<SecretString>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(d)?;
    Ok(opt.map(|s| SecretString::new(s.into())))
}

/// Custom serializer for optional inline secret fields. Always emits
/// `none`, regardless of whether the value is `Some` or `None`. This
/// guarantees that no plaintext credential reaches a serialized
/// representation (TOML, JSON, postcard, etc.) via the auto-derived
/// `Serialize` on [`AuthConfig`]. A round-trip through serialize +
/// deserialize is `None`-preserving by design: secrets do not survive
/// a serialize hop, which is the safe failure mode.
fn serialize_redacted_secret<S>(
    _: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_none()
}

/// Auth kind — inferred from context when omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum AuthKind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "bearer")]
    #[default]
    Bearer,
    #[serde(rename = "basic")]
    Basic,
    #[serde(rename = "oauth2")]
    OAuth2,
}

/// Location of an API key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum ApiKeyLocation {
    #[serde(rename = "header")]
    Header { name: String },
    #[serde(rename = "query")]
    Query { name: String },
}

impl Default for ApiKeyLocation {
    fn default() -> Self {
        ApiKeyLocation::Header {
            name: "X-Api-Key".to_string(),
        }
    }
}

/// Per-API auth configuration as deserialized from TOML.
///
/// Not all fields apply to every `AuthKind`; unused fields are ignored.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Auth kind. When omitted the resolver defaults to [`AuthKind::Bearer`].
    #[serde(default)]
    pub kind: Option<AuthKind>,

    /// Inline secret value (insecure, for quick testing only).
    #[serde(
        default,
        deserialize_with = "deserialize_secret_string",
        serialize_with = "serialize_redacted_secret"
    )]
    pub token: Option<SecretString>,

    /// Environment variable name for the token / credential.
    pub token_env: Option<String>,

    /// URL-style secret reference via `hasp`.
    /// e.g. `keyring://spall/github-token`
    pub token_url: Option<String>,

    /// URL-style secret reference for the password in Basic auth via `hasp`.
    /// e.g. `keyring://spall/my-api-password`
    pub password_url: Option<String>,

    /// URL-style secret reference for the OAuth2 client secret via `hasp`.
    /// e.g. `env://SPALL_CLIENT_SECRET`
    pub client_secret_url: Option<String>,

    // API key specific fields
    /// `"header"` or `"query"`.
    pub location: Option<String>,
    /// Header name when `location` is `"header"`.
    pub header_name: Option<String>,
    /// Query parameter name when `location` is `"query"`.
    pub query_name: Option<String>,

    // Basic auth specific fields
    pub username: Option<String>,
    /// Inline password (insecure, prefer `password_env`).
    #[serde(
        default,
        deserialize_with = "deserialize_secret_string",
        serialize_with = "serialize_redacted_secret"
    )]
    pub password: Option<SecretString>,
    /// Environment variable holding the password.
    pub password_env: Option<String>,

    // OAuth2 specific fields
    pub client_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_secret_string",
        serialize_with = "serialize_redacted_secret"
    )]
    pub client_secret: Option<SecretString>,
    pub auth_url: Option<String>,
    pub scopes: Option<Vec<String>>,

    // Legacy Wave 1–2 keyring fields (deprecated, map to `token_url` when
    // the `hasp` feature is enabled).
    #[serde(default)]
    pub keyring_service: Option<String>,
    #[serde(default)]
    pub keyring_user: Option<String>,
}

/// Resolved authentication material ready for HTTP request injection.
///
/// Produced by the auth resolution chain in `spall-cli`.
#[derive(Debug, Clone)]
pub enum ResolvedAuth {
    Bearer(SecretString),
    ApiKey {
        key: SecretString,
        location: ApiKeyLocation,
    },
    Basic {
        username: String,
        password: SecretString,
    },
    OAuth2(SecretString),
}

impl ResolvedAuth {
    /// Return a display label for the auth kind.
    pub fn kind_label(&self) -> &'static str {
        match self {
            ResolvedAuth::Bearer(_) => "bearer",
            ResolvedAuth::ApiKey { .. } => "api_key",
            ResolvedAuth::Basic { .. } => "basic",
            ResolvedAuth::OAuth2(_) => "oauth2",
        }
    }
}

/// Build the legacy Wave 1–2 default env var name for an API.
///
/// `SPALL_<API>_TOKEN` where hyphens become underscores.
pub fn default_token_env(api_name: &str) -> String {
    format!("SPALL_{}_TOKEN", api_name.to_uppercase().replace('-', "_"))
}
