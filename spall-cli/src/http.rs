use reqwest::{Client, ClientBuilder, redirect::Policy};
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
    let mut cfg = HttpConfig::default();

    let get_timeout = || p2.get_one::<u64>("spall-timeout").or(p1.get_one::<u64>("spall-timeout"));
    if let Some(timeout) = get_timeout() {
        cfg.timeout = Duration::from_secs(*timeout);
    }

    let get_retry = || p2.get_one::<u8>("spall-retry").or(p1.get_one::<u8>("spall-retry"));
    if let Some(retry) = get_retry() {
        cfg.retry = *retry;
    }

    cfg.follow_redirects = p2.get_flag("spall-follow") || p1.get_flag("spall-follow");

    let get_max = || p2.get_one::<usize>("spall-max-redirects").or(p1.get_one::<usize>("spall-max-redirects"));
    if let Some(max) = get_max() {
        cfg.max_redirects = *max;
    }

    cfg.insecure = p2.get_flag("spall-insecure") || p1.get_flag("spall-insecure");

    let get_cert = || p2.get_one::<String>("spall-ca-cert").or(p1.get_one::<String>("spall-ca-cert"));
    if let Some(cert) = get_cert() {
        cfg.ca_cert = Some(cert.clone());
    }

    let get_proxy = || p2.get_one::<String>("spall-proxy").or(p1.get_one::<String>("spall-proxy"));
    if let Some(proxy) = get_proxy() {
        cfg.proxy = Some(proxy.clone());
    }

    let get_server = || p2.get_one::<String>("spall-server").or(p1.get_one::<String>("spall-server"));
    if let Some(server) = get_server() {
        cfg.base_url_override = Some(server.clone());
    }

    let get_auth = || p2.get_one::<String>("spall-auth").or(p1.get_one::<String>("spall-auth"));
    if let Some(auth) = get_auth() {
        cfg.auth_header = Some(auth.clone());
    }

    let get_headers = || p2.get_many::<String>("spall-header").or(p1.get_many::<String>("spall-header"));
    if let Some(headers) = get_headers() {
        for h in headers {
            if let Some((k, v)) = h.split_once(':') {
                cfg.custom_headers
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }

    cfg
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

    // TODO(Wave 1): proxy, CA cert, default headers, base URL override.
    // TODO(Wave 1): TLS cert configuration (client cert + key).

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
