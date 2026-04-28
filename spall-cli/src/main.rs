//! spall-cli: Binary entry point. Two-phase clap parse and dispatch.

mod commands;
mod completions;
mod discover;
mod execute;
mod fetch;
mod filter;
mod history;
mod http;
mod output;
mod paginate;
mod preview;
mod repeat;
mod validate;
mod auth;

use clap::{Arg, ArgAction, ArgMatches, Command};
use miette::Diagnostic;
use spall_config::registry::{ApiEntry, ApiRegistry};
use thiserror::Error;

/// Exit codes.
pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_NETWORK: i32 = 2;
pub const EXIT_SPEC: i32 = 3;
pub const EXIT_HTTP_4XX: i32 = 4;
pub const EXIT_HTTP_5XX: i32 = 5;
pub const EXIT_VALIDATION: i32 = 10;

/// CLI-specific errors with miette diagnostics.
#[derive(Error, Diagnostic, Debug)]
pub enum SpallCliError {
    #[error("Failed to load spec for '{api}'")]
    #[diagnostic(help("Check the URL or run `spall api refresh {api}`.
If this API requires a VPN, ensure you're connected."))]
    SpecLoadFailed {
        api: String,
        source: String,
        #[source]
        cause: spall_core::error::SpallCoreError,
    },

    #[error("Config error")]
    Config(#[from] spall_config::error::SpallConfigError),

    #[error("Usage error: {0}")]
    Usage(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("HTTP client error: {0}")]
    HttpClient(String),

    #[error("HTTP {0} client error")]
    Http4xx(u16),

    #[error("HTTP {0} server error")]
    Http5xx(u16),

    #[error("Validation failed")]
    ValidationFailed,

    #[error("Auth resolution failed for '{api}': {message}")]
    AuthResolution { api: String, message: String },
}

impl SpallCliError {
    fn exit_code(&self) -> i32 {
        match self {
            SpallCliError::SpecLoadFailed { .. } => EXIT_SPEC,
            SpallCliError::Config(_) => EXIT_USAGE,
            SpallCliError::Usage(_) => EXIT_USAGE,
            SpallCliError::Network(_) => EXIT_NETWORK,
            SpallCliError::Cache(_) => EXIT_USAGE,
            SpallCliError::HttpClient(_) => EXIT_NETWORK,
            SpallCliError::Http4xx(_) => EXIT_HTTP_4XX,
            SpallCliError::Http5xx(_) => EXIT_HTTP_5XX,
            SpallCliError::ValidationFailed => EXIT_VALIDATION,
            SpallCliError::AuthResolution { .. } => EXIT_USAGE,
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{:?}", e);
        let code = match e.downcast_ref::<SpallCliError>() {
            Some(err) => err.exit_code(),
            None => EXIT_USAGE,
        };
        std::process::exit(code);
    }
}

async fn run() -> miette::Result<()> {
    let registry = ApiRegistry::load().map_err(SpallCliError::Config)?;
    let args: Vec<String> = std::env::args().collect();

    let cache_dir = dirs::cache_dir()
        .map(|d| d.join("spall"))
        .unwrap_or_else(|| spall_config::sources::config_dir().join("cache"));
    std::fs::create_dir_all(&cache_dir).ok();

    // Fast path: `spall <api> --help` / `spall <api> -h` bypasses Phase 1
    // because Phase 1 stubs have disable_help_flag(true) and would error.
    if let Some(api_name) = detect_api_help(&registry, &args) {
        return show_api_help(&registry, &api_name, &cache_dir).await;
    }

    let mut phase1 = build_phase1(&registry);
    let phase1_matches = match phase1.clone().try_get_matches_from(&args) {
        Ok(m) => m,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            e.print().map_err(|e| SpallCliError::Usage(e.to_string()))?;
            return Ok(());
        }
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayVersion => {
            e.print().map_err(|e| SpallCliError::Usage(e.to_string()))?;
            return Ok(());
        }
        Err(e) => return Err(SpallCliError::Usage(e.to_string()).into()),
    };

    // --spall-repeat: replay a request from history.
    if phase1_matches.get_flag("spall-repeat") {
        let entry_id = match phase1_matches.subcommand() {
            Some(("history", sub)) => {
                // "spall history show 42 --spall-repeat"
                if let Some(("show", show_sub)) = sub.subcommand() {
                    show_sub.get_one::<String>("id").and_then(|s| s.parse().ok())
                } else {
                    None
                }
            }
            _ => None,
        };
        return crate::repeat::replay(&cache_dir,
            entry_id,
        ).await.map_err(Into::into);
    }

    match phase1_matches.subcommand() {
        Some(("api", sub)) => {
            // --spall-repeat on `api discover` is a no-op, but let’s handle it
            // consistently.  If the subcommand is "discover" we ignore the flag.
            commands::api::handle_api_management(sub, &cache_dir).await
        }
        Some(("auth", sub)) => commands::auth::handle_auth(sub).await,
        Some(("completions", sub)) => {
            let shell = sub.get_one::<String>("shell")
                .ok_or_else(|| SpallCliError::Usage("shell argument required".to_string()))?;
            let script = completions::generate_completions(shell)?;
            println!("{}", script);
            Ok(())
        }
        Some(("history", sub)) => commands::history::handle_history(sub, &cache_dir),
        Some(("__complete", sub)) => handle_complete(sub, &registry).await,
        Some((api_name, api_matches)) => {
            let remaining = execute::collect_remaining_args(api_matches);
            handle_api_operation(api_name, remaining, &registry, &phase1_matches, &cache_dir).await
        }
        None => {
            phase1.print_help().map_err(|e| SpallCliError::Usage(e.to_string()))?;
            println!();
            Ok(())
        }
    }
}

async fn handle_complete(
    matches: &ArgMatches,
    registry: &ApiRegistry,
) -> miette::Result<()> {
    let api_name = matches.get_one::<String>("api")
        .ok_or_else(|| SpallCliError::Usage("API name required".to_string()))?;
    let word = matches.get_one::<String>("word").map(|s| s.as_str()).unwrap_or("");

    let entry = match registry.resolve_profile(api_name, None) {
        Some(e) => e,
        None => {
            return Ok(());
        }
    };

    let cache_dir = dirs::cache_dir()
        .map(|d| d.join("spall"))
        .unwrap_or_else(|| spall_config::sources::config_dir().join("cache"));

    let proxy = crate::http::resolve_env_proxy();
    let raw = match fetch::load_raw(&entry.source, &cache_dir, proxy.as_deref()).await {
        Ok(b) => b,
        Err(_) => {
            if let Some(index) = spall_core::cache::load_cached_index(&entry.source, &cache_dir
            ) {
                for op in &index.operations {
                    if op.operation_id.starts_with(word) {
                        println!("{}", op.operation_id);
                    }
                }
            }
            return Ok(());
        }
    };

    let spec = match spall_core::cache::load_or_resolve(&entry.source, &raw, &cache_dir
    ) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    for op in &spec.operations {
        if op.operation_id.starts_with(word) {
            println!("{}", op.operation_id);
        }
        for param in &op.parameters {
            let id = format!("{}-{}", param.location.as_str(), param.name);
            if id.starts_with(word) {
                println!("{}", id);
            }
        }
    }

    Ok(())
}

/// Detect `spall <api> --help` / `spall <api> -h` before Phase 1 parsing.
fn detect_api_help(registry: &ApiRegistry, args: &[String]) -> Option<String> {
    if args.len() >= 3 {
        let api_name = &args[1];
        let next = &args[2];
        if (next == "--help" || next == "-h") && registry.find(api_name).is_some() {
            return Some(api_name.clone());
        }
    }
    None
}

/// Show help for an API by loading its spec and building Phase 2.
async fn show_api_help(
    registry: &ApiRegistry,
    api_name: &str,
    cache_dir: &std::path::Path,
) -> miette::Result<()> {
    let entry = registry.find(api_name).unwrap();
    let proxy = crate::http::resolve_env_proxy();
    let raw = match fetch::load_raw(&entry.source, cache_dir, proxy.as_deref()).await {
        Ok(bytes) => bytes,
        Err(e) => {
            // Degraded help from cache
            if let Some(index) = spall_core::cache::load_cached_index(&entry.source, cache_dir) {
                eprintln!(
                    "⚠  Could not load spec for '{}'. Showing cached operation list from {}.",
                    api_name, index.cached_at
                );
                let mut phase2 =
                    spall_core::command::build_operations_cmd_from_index(api_name, &index);
                for arg in spall_global_args() {
                    phase2 = phase2.arg(arg);
                }
                phase2.print_help().map_err(|e| SpallCliError::Usage(e.to_string()))?;
                println!();
                return Ok(());
            } else {
                return Err(SpallCliError::SpecLoadFailed {
                    api: api_name.to_string(),
                    source: entry.source.clone(),
                    cause: spall_core::error::SpallCoreError::InvalidSource(e.to_string()),
                }
                .into());
            }
        }
    };

    let spec = match spall_core::cache::load_or_resolve(
        &entry.source,
        &raw,
        cache_dir,
    ) {
        Ok(spec) => spec,
        Err(e) => {
            if let Some(index) = spall_core::cache::load_cached_index(&entry.source, cache_dir
            ) {
                eprintln!(
                    "⚠  Could not load spec for '{}'. Showing cached operation list from {}.",
                    api_name, index.cached_at
                );
                let mut phase2 =
                    spall_core::command::build_operations_cmd_from_index(api_name, &index);
                for arg in spall_global_args() {
                    phase2 = phase2.arg(arg);
                }
                phase2.print_help().map_err(|e| SpallCliError::Usage(e.to_string()))?;
                println!();
                return Ok(());
            } else {
                return Err(SpallCliError::SpecLoadFailed {
                    api: api_name.to_string(),
                    source: entry.source.clone(),
                    cause: e,
                }
                .into());
            }
        }
    };

    let mut phase2 = spall_core::command::build_operations_cmd(api_name, &spec);
    for arg in spall_global_args() {
        phase2 = phase2.arg(arg);
    }
    phase2.print_help().map_err(|e| SpallCliError::Usage(e.to_string()))?;
    println!();
    Ok(())
}

/// Phase 2: load spec, build command tree, parse remaining args, execute.
async fn handle_api_operation(
    api_name: &str,
    remaining: Vec<String>,
    registry: &ApiRegistry,
    phase1_matches: &ArgMatches,
    cache_dir: &std::path::Path,
) -> miette::Result<()> {
    let profile = phase1_matches.get_one::<String>("profile").map(|s| s.as_str());
    let entry = registry
        .resolve_profile(api_name, profile)
        .ok_or_else(|| SpallCliError::Usage(format!("Unknown API: {}", api_name)))?;

    let proxy = crate::http::resolve_proxy(
        &entry,
        &registry.defaults,
        phase1_matches,
        &clap::ArgMatches::default(),
    );
    let raw = fetch::load_raw(&entry.source, cache_dir, proxy.as_deref())
        .await
        .map_err(|e| SpallCliError::SpecLoadFailed {
            api: api_name.to_string(),
            source: entry.source.clone(),
            cause: spall_core::error::SpallCoreError::InvalidSource(e.to_string()),
        })?;

    let spec = spall_core::cache::load_or_resolve(&entry.source, &raw, cache_dir)
        .map_err(|e| SpallCliError::SpecLoadFailed {
            api: api_name.to_string(),
            source: entry.source.clone(),
            cause: e,
        })?;

    let mut phase2 = spall_core::command::build_operations_cmd(api_name, &spec);
    for arg in spall_global_args() {
        phase2 = phase2.arg(arg);
    }

    // Prepend API name so clap sees the correct command name as argv[0].
    let mut phase2_args = vec![api_name.to_string()];
    phase2_args.extend(remaining);

    let phase2_matches = match phase2.try_get_matches_from(&phase2_args) {
        Ok(m) => m,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            // Clap returns a DisplayHelp error; print it ourselves.
            e.print().map_err(|e| SpallCliError::Usage(e.to_string()))?;
            return Ok(());
        }
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayVersion => {
            // Clap already printed version to stdout.
            return Ok(());
        }
        Err(e) => {
            return Err(SpallCliError::Usage(e.to_string()).into());
        }
    };

    let (tag_or_op, op_matches) = phase2_matches.subcommand().ok_or_else(|| {
        SpallCliError::Usage("No operation specified. Use --help to list operations.".to_string())
    })?;

    // Phase 2 structure may be flat (single tag) or nested (multiple tags).
    // Try direct operation match first.
    if let Some(op) = spec.operations.iter().find(|o| o.operation_id == tag_or_op) {
        return execute::execute_operation(op, &spec, &entry, op_matches, phase1_matches, cache_dir, &registry.defaults)
            .await
            .map_err(Into::into);
    }

    // If not found directly, look for a tag subcommand.
    if let Some(_tag_matches) = phase2_matches.subcommand_matches(tag_or_op) {
        let (op_name, inner_matches) = _tag_matches.subcommand().ok_or_else(|| {
            SpallCliError::Usage("No operation specified. Use --help to list operations.".to_string())
        })?;

        let op = spec
            .operations
            .iter()
            .find(|o| o.operation_id == op_name)
            .ok_or_else(|| {
                SpallCliError::Usage(format!("Unknown operation: {}", op_name))
            })?;

        return execute::execute_operation(op, &spec, &entry, inner_matches, phase1_matches, cache_dir, &registry.defaults)
            .await
            .map_err(Into::into);
    }

    Err(SpallCliError::Usage(format!(
        "Unknown operation: {}",
        tag_or_op
    )).into())
}

/// Build Phase 1 command tree from the API registry.
///
/// Each API is registered as a stub subcommand with
/// `allow_external_subcommands(true)` and `disable_help_flag(true)` so
/// that `--help` falls through to Phase 2.
fn build_phase1(registry: &ApiRegistry) -> Command {
    let mut root = Command::new("spall")
        .about("Break free. Hit the endpoint.")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(api_management_cmd())
        .subcommand(auth_cmd())
        .subcommand(history_cmd())
        .subcommand(
            Command::new("completions")
                .about("Generate shell completion scripts")
                .arg(Arg::new("shell").required(true).help("Shell: bash, zsh, or fish")),
        )
        .subcommand(
            Command::new("__complete")
                .hide(true)
                .about("Internal completion helper")
                .arg(Arg::new("api").required(true).help("API name"))
                .arg(Arg::new("word").help("Partial word to complete")),
        )
        .args(spall_global_args());

    for entry in &registry.apis {
        root = root.subcommand(
            Command::new(entry.name.clone())
                .about(entry.source.to_string())
                .allow_external_subcommands(true)
                .disable_help_flag(true)
                .disable_version_flag(true)
                .args(spall_global_args()),
        );
    }

    root
}

/// Build the `spall auth` subcommand tree.
fn auth_cmd() -> Command {
    Command::new("auth")
        .about("Authentication commands")
        .subcommand(
            Command::new("status")
                .about("Show auth status for an API")
                .arg(Arg::new("api").required(true).help("API name")),
        )
        .subcommand(
            Command::new("login")
                .about("Authenticate with an API (stub for PKCE)")
                .arg(Arg::new("api").required(true).help("API name")),
        )
}

/// Build the `spall history` subcommand tree.
fn history_cmd() -> Command {
    Command::new("history")
        .about("Request/response history")
        .subcommand(Command::new("list").about("List recent requests"))
        .subcommand(
            Command::new("show")
                .about("Show request details")
                .arg(Arg::new("id").required(true).help("Request ID")),
        )
        .subcommand(Command::new("clear").about("Clear history"))
}

/// Build the `spall api` management subcommand.
fn api_management_cmd() -> Command {
    Command::new("api")
        .about("Manage registered APIs")
        .subcommand(
            Command::new("add")
                .about("Register a new API")
                .arg(Arg::new("name").required(true).help("API name"))
                .arg(Arg::new("source").required(true).help("Spec file path or URL")),
        )
        .subcommand(Command::new("list").about("List registered APIs"))
        .subcommand(
            Command::new("remove")
                .about("Unregister an API")
                .arg(Arg::new("name").required(true).help("API name")),
        )
        .subcommand(
            Command::new("refresh")
                .about("Refresh cached specs")
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Refresh all APIs"),
                )
                .arg(Arg::new("name").help("API name")),
        )
        .subcommand(
            Command::new("discover")
                .about("Discover and register an API from a URL (RFC 8631)")
                .arg(Arg::new("url").required(true).help("Base URL to probe")),
        )
}

/// Register all `--spall-*` global flags.
fn spall_global_args() -> Vec<Arg> {
    vec![
        Arg::new("spall-repeat")
            .long("spall-repeat")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Replay the most recent request from history"),
        Arg::new("spall-output")
            .long("spall-output")
            .short('O')
            .global(true)
            .help("Output format, or @file to save response"),
        Arg::new("spall-verbose")
            .long("spall-verbose")
            .short('v')
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Print request/response headers to stderr"),
        Arg::new("spall-debug")
            .long("spall-debug")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Wire-level debug logging (redacts secrets)"),
        Arg::new("spall-dry-run")
            .long("spall-dry-run")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Print curl equivalent without executing"),
        Arg::new("spall-header")
            .long("spall-header")
            .short('H')
            .action(ArgAction::Append)
            .global(true)
            .help("Inject a non-sensitive header (repeatable)"),
        Arg::new("spall-auth")
            .long("spall-auth")
            .short('A')
            .global(true)
            .help("Pass-through auth token/header (e.g., Bearer <token>)"),
        Arg::new("spall-server")
            .long("spall-server")
            .short('s')
            .global(true)
            .help("Override base URL for this request"),
        Arg::new("spall-timeout")
            .long("spall-timeout")
            .short('t')
            .default_value("30")
            .value_parser(clap::value_parser!(u64))
            .global(true)
            .help("Request/spec fetch timeout in seconds (default: 30)"),
        Arg::new("spall-retry")
            .long("spall-retry")
            .default_value("1")
            .value_parser(clap::value_parser!(u8).range(..=3))
            .global(true)
            .help("Retry count for failed requests (default: 1, max: 3)"),
        Arg::new("spall-follow")
            .long("spall-follow")
            .short('L')
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Follow HTTP redirects (default: off)"),
        Arg::new("spall-max-redirects")
            .long("spall-max-redirects")
            .default_value("10")
            .value_parser(clap::value_parser!(usize))
            .global(true)
            .help("Maximum number of redirects to follow (default: 10)"),
        Arg::new("spall-time")
            .long("spall-time")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Include request/response timing in verbose output"),
        Arg::new("spall-download")
            .long("spall-download")
            .short('o')
            .global(true)
            .help("Save response body to file"),
        Arg::new("spall-insecure")
            .long("spall-insecure")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Skip TLS certificate verification"),
        Arg::new("spall-ca-cert")
            .long("spall-ca-cert")
            .global(true)
            .help("Path to custom CA certificate"),
        Arg::new("spall-no-proxy")
            .long("spall-no-proxy")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Disable proxy for this request"),
        Arg::new("spall-proxy")
            .long("spall-proxy")
            .global(true)
            .help("HTTP/SOCKS proxy URL"),
        Arg::new("spall-content-type")
            .long("spall-content-type")
            .short('c')
            .global(true)
            .help("Override request content type"),
        Arg::new("spall-paginate")
            .long("spall-paginate")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Auto-follow Link header pagination"),
        Arg::new("spall-preview")
            .long("spall-preview")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Show resolved request without sending"),
        Arg::new("profile")
            .long("profile")
            .global(true)
            .help("Active config profile (e.g., staging, production)"),
        Arg::new("spall-filter")
            .long("filter")
            .global(true)
            .help("JMESPath filter expression for JSON responses"),
    ]
}
