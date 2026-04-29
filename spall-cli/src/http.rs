use reqwest::{Client, ClientBuilder, Proxy, redirect::Policy};
use crate::matches::MergedMatches;
use std::collections::HashMap;
use std::time::Duration;

/// Configuration for building the HTTP client.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub timeout: Duration,
    pub retry: u8,
    pub follow_redirects: bool,
    pub max_redirects: usize,
    pub insecure: bool,
    pub ca_cert: Option<String>,
    pub proxy: Option<String>,
    pub no_proxy: bool,
    pub base_url_override: Option<String>,
    pub default_headers: HashMap<String, String>,
    pub user_agent: String,
    pub auth_header: Option<String>,
    pub custom_headers: Vec<(String, String)>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry: 1,
            follow_redirects: false,
            max_redirects: 10,
            insecure: false,
            ca_cert: None,
            proxy: None,
            no_proxy: false,
            base_url_override: None,
            default_headers: HashMap::new(),
            user_agent: format!("spall/{}", env!("CARGO_PKG_VERSION")),
            auth_header: None,
            custom_headers: Vec::new(),
        }
    }
}

/// Build `HttpConfig` from clap matches, checking Phase 1 then Phase 2.
pub fn config_from_matches(p1: &clap::ArgMatches, p2: &clap::ArgMatches) -> HttpConfig {
    let m = MergedMatches { phase1: p1, phase2: p2 };
    let mut cfg = HttpConfig::default();

    if let Some(timeout) = m.get_one::<u64>("spall-timeout") {
        cfg.timeout = Duration::from_secs(timeout);
    }

    if let Some(retry) = m.get_one::<u8>("spall-retry") {
        cfg.retry = retry;
    }

    cfg.follow_redirects = m.get_flag("spall-follow");

    if let Some(max) = m.get_one::<usize>("spall-max-redirects") {
        cfg.max_redirects = max;
    }

    cfg.insecure = m.get_flag("spall-insecure");

    if let Some(cert) = m.get_one::<String>("spall-ca-cert") {
        cfg.ca_cert = Some(cert);
    }

    cfg.no_proxy = m.get_flag("spall-no-proxy");

    if let Some(proxy) = m.get_one::<String>("spall-proxy") {
        cfg.proxy = Some(proxy);
    }

    if let Some(server) = m.get_one::<String>("spall-server") {
        cfg.base_url_override = Some(server);
    }

    if let Some(auth) = m.get_one::<String>("spall-auth") {
        cfg.auth_header = Some(auth);
    }

    if let Some(headers) = m.get_many::<String>("spall-header") {
        for h in headers {
            if let Some((k, v)) = h.split_once(':') {
                cfg.custom_headers
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }

    cfg
}

/// Resolve the effective proxy URL for a request following the priority chain.
///
/// 1. `--spall-no-proxy` → None
/// 2. `--spall-proxy`
/// 3. Per-API config `proxy`
/// 4. Global default `proxy`
/// 5. Environment variables `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`
/// 6. None
#[must_use]
pub fn resolve_proxy(
    entry: &spall_config::registry::ApiEntry,
    global_defaults: &spall_config::sources::GlobalDefaults,
    p1: &clap::ArgMatches,
    p2: &clap::ArgMatches,
) -> Option<String> {
    let m = MergedMatches { phase1: p1, phase2: p2 };

    if m.get_flag("spall-no-proxy") {
        return None;
    }

    if let Some(proxy) = m.get_one::<String>("spall-proxy") {
        return Some(proxy);
    }

    if entry.proxy.is_some() {
        return entry.proxy.clone();
    }

    if let Some(proxy) = env_proxy() {
        return Some(proxy);
    }

    if global_defaults.proxy.is_some() {
        return global_defaults.proxy.clone();
    }

    None
}

/// Resolve proxy from environment variables only.
#[must_use]
pub fn resolve_env_proxy() -> Option<String> {
    env_proxy()
}

fn env_proxy() -> Option<String> {
    std::env::var("HTTPS_PROXY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("HTTP_PROXY")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            std::env::var("ALL_PROXY")
                .ok()
                .filter(|s| !s.is_empty())
        })
}

/// Build a `reqwest::Client` from `HttpConfig`.
pub fn build_http_client(config: &HttpConfig) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder();

    builder = builder.timeout(config.timeout);
    builder = builder.user_agent(&config.user_agent);

    if config.follow_redirects {
        builder = builder.redirect(Policy::limited(config.max_redirects));
    } else {
        builder = builder.redirect(Policy::none());
    }

    if config.insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }

    // TODO: CA cert, client cert + key.

    if let Some(proxy_url) = &config.proxy {
        let proxy = Proxy::all(proxy_url)?
            .no_proxy(reqwest::NoProxy::from_env());
        builder = builder.proxy(proxy);
    }

    builder.build()
}

/// Build a `reqwest::Client` for non-interactive fetches (spec loading, discovery, etc.).
///
/// Uses a 30-second timeout and limited redirects. If `proxy_url` is provided,
/// it is applied unless the destination host is matched by `NO_PROXY`.
pub fn build_fetch_client(proxy_url: Option<&str>) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::limited(5));

    if let Some(url) = proxy_url {
        let proxy = Proxy::all(url)?
            .no_proxy(reqwest::NoProxy::from_env());
        builder = builder.proxy(proxy);
    }

    builder.build()
}

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u8,
    pub base_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 1,
            base_delay_ms: 500,
        }
    }
}
