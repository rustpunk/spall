use thiserror::Error;

/// Errors originating in spall-config.
#[derive(Error, Debug)]
pub enum SpallConfigError {
    #[error("config file not found: {0}")]
    ConfigNotFound(String),

    #[error("failed to parse config TOML: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("failed to serialize config TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("credential resolution failed for api '{api}': {detail}")]
    CredentialResolution { api: String, detail: String },

    #[error("invalid API name: {0}")]
    InvalidApiName(String),
}
