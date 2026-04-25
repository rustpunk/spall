use crate::error::SpallConfigError;
use crate::sources::{load_global_config, scan_api_configs, scan_spec_dirs};
use std::path::PathBuf;

/// Lightweight index of registered APIs.
///
/// Constructed in <5ms for 50 APIs — only scans config, never parses specs.
#[derive(Debug, Clone)]
pub struct ApiRegistry {
    pub apis: Vec<ApiEntry>,
}

/// An entry in the API registry.
#[derive(Debug, Clone)]
pub struct ApiEntry {
    /// User-facing API name (e.g., "petstore", "my-internal-api").
    pub name: String,
    /// Source: file path or URL to the spec.
    pub source: String,
    /// Optional path to per-API config file.
    pub config_path: Option<PathBuf>,
    /// Optional base URL override from config.
    pub base_url: Option<String>,
    /// Default headers from config.
    pub default_headers: Vec<(String, String)>,
    /// Auth configuration reference.
    pub auth: Option<AuthConfig>,
}

/// Auth configuration for an API (references only; no raw secrets).
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Name of the environment variable holding the token.
    pub token_env: Option<String>,
    /// Keyring service name.
    pub keyring_service: Option<String>,
    /// Keyring user/account name.
    pub keyring_user: Option<String>,
}

impl ApiRegistry {
    /// Build the registry from config sources.
    ///
    /// Scans `config.toml`, `apis/*.toml`, and `spec_dirs`.
    /// Priority (highest → lowest): `apis/*.toml`, `[[api]]` inline entries, `spec_dirs`.
    pub fn load() -> Result<ApiRegistry, SpallConfigError> {
        let global = load_global_config()?;
        let api_files = scan_api_configs()?;
        let inline = global.inline_apis;
        let scanned = scan_spec_dirs(&global.spec_dirs)?;

        // Priority: api_files > inline > scanned.  Lower-priority entries
        // with duplicate names are discarded.
        let mut seen = std::collections::HashSet::new();
        let mut apis: Vec<ApiEntry> = Vec::with_capacity(api_files.len() + inline.len() + scanned.len());

        for e in api_files {
            if seen.insert(e.name.clone()) {
                apis.push(e);
            }
        }
        for e in inline {
            if seen.insert(e.name.clone()) {
                apis.push(e);
            }
        }
        for e in scanned {
            if seen.insert(e.name.clone()) {
                apis.push(e);
            }
        }

        Ok(ApiRegistry { apis })
    }

    /// Persist a new API to the registry by creating `~/.config/spall/apis/{name}.toml`.
    pub fn add_api(
        name: &str,
        source: &str,
    ) -> Result<(), SpallConfigError> {
        validate_name(name)?;
        let dir = crate::sources::config_dir().join("apis");
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}.toml", name));
        let content = format!("source = \"{}\"\n", source.replace('"', "\\\""));
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Remove an API's config file from `~/.config/spall/apis/`.
    pub fn remove_api(name: &str) -> Result<(), SpallConfigError> {
        let path = crate::sources::config_dir().join("apis").join(format!("{}.toml", name));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Find an entry by name.
    pub fn find(&self, name: &str) -> Option<&ApiEntry> {
        self.apis.iter().find(|e| e.name == name)
    }
}

fn validate_name(name: &str) -> Result<(), SpallConfigError> {
    // Reject names that start with a dash or are empty.
    if name.is_empty() {
        return Err(SpallConfigError::InvalidApiName(
            "API name cannot be empty".to_string()),
        );
    }
    if name.starts_with('-') {
        return Err(SpallConfigError::InvalidApiName(
            "API name cannot start with '-'".to_string(),
        ));
    }
    // Disallow shell/meta characters that would break CLI usage.
    let bad: &[char] = &['/', '\\', '\'', '"', '\n', '\r', '\t'];
    if name.contains(bad) {
        return Err(SpallConfigError::InvalidApiName(format!(
            "API name '{}' contains invalid characters",
            name
        )));
    }
    Ok(())
}
