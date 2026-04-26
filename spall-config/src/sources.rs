use crate::auth::AuthConfig;
use crate::error::SpallConfigError;
use crate::registry::ApiEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// TOML-serializable structures (internal to this module)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct SpallConfig {
    #[serde(default)]
    api: Vec<InlineApi>,
    #[serde(default)]
    spec_dirs: Vec<String>,
    #[serde(default)]
    defaults: Option<Defaults>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct InlineApi {
    name: String,
    spec: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct Defaults {
    output: Option<String>,
    color: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ApiToml {
    source: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    auth: Option<AuthConfig>,
    #[serde(default)]
    profile: HashMap<String, ProfileToml>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ProfileToml {
    base_url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    auth: Option<AuthConfig>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve the spall config directory (`~/.config/spall`).
pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("spall")
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    }
}

/// Load the global `config.toml` from the XDG config directory.
///
/// If the file does not exist, returns an empty `GlobalConfig`.
pub fn load_global_config() -> Result<GlobalConfig, SpallConfigError> {
    let path = config_dir().join("config.toml");
    if !path.exists() {
        return Ok(GlobalConfig {
            inline_apis: Vec::new(),
            spec_dirs: Vec::new(),
            defaults: GlobalDefaults::default(),
        });
    }

    let text = std::fs::read_to_string(&path)?;
    let cfg: SpallConfig = toml::from_str(&text)?;

    let inline_apis = cfg
        .api
        .into_iter()
        .map(|a| ApiEntry {
            name: a.name.clone(),
            source: a.spec,
            config_path: None,
            base_url: None,
            default_headers: Vec::new(),
            auth: None,
            profiles: std::collections::HashMap::new(),
        })
        .collect();

    let spec_dirs = cfg.spec_dirs.into_iter().map(|s| expand_tilde(&s)).collect();

    let defaults = GlobalDefaults {
        output: cfg.defaults.as_ref().and_then(|d| d.output.clone()),
        color: cfg.defaults.as_ref().and_then(|d| d.color.clone()),
    };

    Ok(GlobalConfig {
        inline_apis,
        spec_dirs,
        defaults,
    })
}

/// Scan `~/.config/spall/apis/` for per-API `.toml` files.
///
/// Each file is named `{api-name}.toml`; the stem becomes the API name.
pub fn scan_api_configs() -> Result<Vec<ApiEntry>, SpallConfigError> {
    let dir = config_dir().join("apis");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "toml").unwrap_or(false) {
            if let Some(name) = derive_name_from_filename(&path) {
                let text = std::fs::read_to_string(&path)?;
                let cfg: ApiToml = toml::from_str(&text)?;

                let headers: Vec<(String, String)> = cfg.headers.into_iter().collect();

                // Convert legacy keyring fields to token_url when possible.
                let mut auth = cfg.auth;
                if let Some(ref mut a) = auth {
                    if a.token_url.is_none() {
                        if let (Some(svc), Some(usr)) = (a.keyring_service.clone(), a.keyring_user.clone()) {
                            a.token_url = Some(format!("keyring://{}/{}", svc, usr));
                        }
                    }
                }

                let profiles: std::collections::HashMap<String, crate::registry::ProfileConfig> = cfg
                    .profile
                    .into_iter()
                    .map(|(name, p)| {
                        let mut profile_auth = p.auth;
                        if let Some(ref mut a) = profile_auth {
                            if a.token_url.is_none() {
                                if let (Some(svc), Some(usr)) = (a.keyring_service.clone(), a.keyring_user.clone()) {
                                    a.token_url = Some(format!("keyring://{}/{}", svc, usr));
                                }
                            }
                        }
                        (
                            name,
                            crate::registry::ProfileConfig {
                                base_url: p.base_url,
                                headers: p.headers.into_iter().collect(),
                                auth: profile_auth,
                            },
                        )
                    })
                    .collect();

                entries.push(ApiEntry {
                    name,
                    source: cfg.source,
                    config_path: Some(path),
                    base_url: cfg.base_url,
                    default_headers: headers,
                    auth,
                    profiles,
                });
            }
        }
    }
    Ok(entries)
}

/// Scan `spec_dirs` for raw spec files and derive API names from filenames.
///
/// Supported extensions: `.json`, `.yaml`, `.yml`.
pub fn scan_spec_dirs(dirs: &[PathBuf]) -> Result<Vec<ApiEntry>, SpallConfigError> {
    let mut entries = Vec::new();
    let exts = ["json", "yaml", "yml"];

    for dir in dirs {
        let dir = expand_tilde(&dir.to_string_lossy());
        if !dir.is_dir() {
            continue;
        }
        for item in std::fs::read_dir(&dir)? {
            let item = item?;
            let path = item.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if exts.contains(&ext.to_ascii_lowercase().as_str()) {
                        if let Some(name) = derive_name_from_filename(&path) {
                            entries.push(ApiEntry {
                                name,
                                source: path.to_string_lossy().into_owned(),
                                config_path: None,
                                base_url: None,
                                default_headers: Vec::new(),
                                auth: None,
                                profiles: std::collections::HashMap::new(),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(entries)
}

/// Derive an API name from a spec filename.
///
/// `petstore.json` → `petstore`, `my-internal-api.yaml` → `my-internal-api`.
pub fn derive_name_from_filename(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|s| s.to_string_lossy().replace('_', "-"))
}

/// Parsed global config structure.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub inline_apis: Vec<ApiEntry>,
    pub spec_dirs: Vec<PathBuf>,
    pub defaults: GlobalDefaults,
}

/// Global default settings.
#[derive(Debug, Clone, Default)]
pub struct GlobalDefaults {
    pub output: Option<String>,
    pub color: Option<String>,
}
